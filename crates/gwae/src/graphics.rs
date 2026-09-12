//! Bounded, pane-local subset of the Kitty graphics protocol.
//!
//! This module does no I/O and never opens a client-supplied path. Only direct
//! RGB/RGBA pixels are accepted. A successful acknowledgement means the entire
//! payload and placement have been validated and committed, not merely seen.

use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_CHUNK: usize = 4096;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_IMAGES: usize = 64;
const MAX_PLACEMENTS: usize = 256;
const MAX_DIMENSION: u32 = 16384;

#[derive(Clone, Debug)]
pub struct Source {
    pub width: u32,
    pub height: u32,
    /// 24 for RGB, 32 for RGBA, in sRGB byte order.
    pub format: u8,
    pub pixels: Arc<Vec<u8>>,
    /// Changes on every successful replacement, but not clear/re-display.
    pub revision: u64,
    number: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub image_id: u32,
    pub placement_id: u32,
    pub row: u16,
    pub col: u16,
    /// Source crop, in pixels (already intersected with the source).
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Destination rectangle, in pixels, excluding the within-cell offset.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub z_index: i32,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub replies: Vec<u8>,
    /// Only successful source replacement, never a query or placement update.
    /// The caller invalidates this child ID in its other graphics mode.
    pub committed_image: Option<u32>,
    /// The caller must route this untouched command through its bounded,
    /// ID-remapped legacy Unicode-placeholder path. Includes continuations.
    pub legacy: bool,
    /// An unsupported feature/action was rejected (not a successful probe).
    pub unsupported: bool,
    /// Requested final cursor position, row then column. C=1 yields None.
    /// The caller applies its normal screen/scroll-area cursor clamping.
    pub cursor: Option<(u16, u16)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Invalid,
    Unsupported,
    Missing,
    TooLarge,
    Quota,
}

impl Error {
    fn message(self) -> &'static str {
        match self {
            Self::Invalid => "EINVAL:invalid graphics command",
            Self::Unsupported => "ENOTSUP:unsupported graphics feature",
            Self::Missing => "ENOENT:image not found",
            Self::TooLarge => "E2BIG:graphics resource limit",
            Self::Quota => "ENOSPC:pane graphics quota",
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default)]
struct Control(BTreeMap<u8, String>);

impl Control {
    fn has(&self, key: u8) -> bool {
        self.0.contains_key(&key)
    }

    fn number(&self, key: u8, default: u32) -> Result<u32> {
        match self.0.get(&key) {
            Some(value) if value.bytes().all(|b| b.is_ascii_digit()) => {
                value.parse().map_err(|_| Error::Invalid)
            }
            Some(_) => Err(Error::Invalid),
            None => Ok(default),
        }
    }

    fn letter(&self, key: u8, default: u8) -> Result<u8> {
        match self.0.get(&key) {
            Some(value) if value.len() == 1 => Ok(value.as_bytes()[0]),
            Some(_) => Err(Error::Invalid),
            None => Ok(default),
        }
    }

    fn action(&self) -> Result<u8> {
        self.letter(b'a', b't')
    }

    fn more(&self) -> Result<bool> {
        match self.number(b'm', 0)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Invalid),
        }
    }

    fn continuation(&self) -> bool {
        self.has(b'm') && self.0.keys().all(|key| matches!(key, b'm' | b'q'))
    }

    fn validate(&self, allowed: &[u8]) -> Result<()> {
        if self.0.keys().any(|key| !allowed.contains(key)) {
            return Err(Error::Unsupported);
        }
        if self.has(b'i') && self.has(b'I') {
            return Err(Error::Invalid);
        }
        for key in [b'i', b'I'] {
            if self.has(key) && self.number(key, 0)? == 0 {
                return Err(Error::Invalid);
            }
        }
        if self.number(b'q', 0)? > 2 {
            return Err(Error::Invalid);
        }
        self.number(b'p', 0)?;
        self.more()?;
        Ok(())
    }

    fn reply(&self, id: Option<u32>, error: Option<Error>) -> Outcome {
        let quiet = self.number(b'q', 0).unwrap_or(0);
        let mut outcome = Outcome {
            unsupported: error == Some(Error::Unsupported),
            ..Outcome::default()
        };
        // Kitty only acknowledges identified operations. In particular, tdf's
        // d=a/d=R calls do not read responses, so do not inject an unsolicited OK.
        if quiet == 2 || (quiet == 1 && error.is_none()) || (!self.has(b'i') && !self.has(b'I')) {
            return outcome;
        }
        let i = id.unwrap_or_else(|| self.number(b'i', 0).unwrap_or(0));
        let mut header = format!("i={i}");
        if let Ok(number) = self.number(b'I', 0) {
            if number != 0 {
                header.push_str(&format!(",I={number}"));
            }
        }
        if let Ok(placement) = self.number(b'p', 0) {
            if placement != 0 {
                header.push_str(&format!(",p={placement}"));
            }
        }
        let message = error.map(Error::message).unwrap_or("OK");
        outcome.replies = format!("\x1b_G{header};{message}\x1b\\").into_bytes();
        outcome
    }
}

/// Preserve valid identity/verbosity fields even when another field is bad,
/// so a malformed identified request gets an error instead of timing out.
fn parse(apc: &[u8]) -> (Control, &[u8], Option<Error>) {
    let mut control = Control::default();
    let Some(body) = apc
        .strip_prefix(b"\x1b_G")
        .and_then(|b| b.strip_suffix(b"\x1b\\"))
    else {
        return (control, &[], Some(Error::Invalid));
    };
    let split = body.iter().position(|&b| b == b';').unwrap_or(body.len());
    let payload = body.get(split + 1..).unwrap_or_default();
    if split > 1024 {
        return (control, &[], Some(Error::TooLarge));
    }
    let mut error = None;
    if split > 0 {
        for pair in body[..split].split(|&b| b == b',') {
            if pair.len() < 3
                || pair[1] != b'='
                || !pair[0].is_ascii_alphabetic()
                || !pair[2..]
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
            {
                error = Some(Error::Invalid);
                continue;
            }
            let value = String::from_utf8(pair[2..].to_vec()).expect("validated ASCII");
            if control.0.contains_key(&pair[0]) {
                error = Some(Error::Invalid);
            } else {
                control.0.insert(pair[0], value);
            }
        }
    }
    (control, payload, error)
}

#[derive(Debug)]
struct Transfer {
    control: Control,
    data: Vec<u8>,
    expected: usize,
    error: Option<Error>,
}

impl Transfer {
    fn new(control: Control, error: Option<Error>, image_limit: usize) -> Self {
        let expected = (|| {
            control.validate(b"aifsvtompqIxywhcrCXYUz")?;
            if control.letter(b't', b'd')? != b'd' || control.has(b'o') {
                return Err(Error::Unsupported);
            }
            if control.number(b'U', 0)? != 0 {
                return Err(Error::Unsupported);
            }
            let format = control.number(b'f', 32)?;
            if !matches!(format, 24 | 32) {
                return Err(Error::Unsupported);
            }
            let width = control.number(b's', 0)?;
            let height = control.number(b'v', 0)?;
            if width == 0 || height == 0 {
                return Err(Error::Invalid);
            }
            if width > MAX_DIMENSION || height > MAX_DIMENSION {
                return Err(Error::TooLarge);
            }
            let bytes = (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(format as usize / 8))
                .ok_or(Error::TooLarge)?;
            if bytes > image_limit {
                return Err(Error::TooLarge);
            }
            Ok(bytes)
        })();
        let mut transfer = Self {
            control,
            data: Vec::new(),
            expected: expected.as_ref().copied().unwrap_or(0),
            error: error.or(expected.err()),
        };
        // Reserve exactly once so Vec's geometric growth cannot retain up to
        // twice the advertised per-image byte budget. Pending storage is at
        // most one image in addition to the retained-source quota.
        if transfer.error.is_none() && transfer.data.try_reserve_exact(transfer.expected).is_err() {
            transfer.error = Some(Error::Quota);
        }
        transfer
    }

    fn append(&mut self, payload: &[u8], more: bool) {
        if self.error.is_some() {
            return;
        }
        let result = (|| {
            if payload.len() > MAX_CHUNK {
                return Err(Error::TooLarge);
            }
            let decoded = decode(payload, !more)?;
            if decoded.len() > self.expected.saturating_sub(self.data.len()) {
                return Err(Error::Invalid);
            }
            self.data.extend_from_slice(&decoded);
            Ok(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            self.data = Vec::new();
        }
    }
}

/// Strict RFC 4648 base64, with no whitespace, alternate alphabets or padding
/// in a non-final chunk. Chunks are independently 4-byte aligned by protocol.
fn decode(encoded: &[u8], final_chunk: bool) -> Result<Vec<u8>> {
    fn digit(byte: u8) -> Result<u8> {
        match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(Error::Invalid),
        }
    }
    if encoded.len() % 4 != 0 {
        return Err(Error::Invalid);
    }
    let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
    for (index, chunk) in encoded.chunks_exact(4).enumerate() {
        let a = digit(chunk[0])?;
        let b = digit(chunk[1])?;
        let last = final_chunk && (index + 1) * 4 == encoded.len();
        decoded.push(a << 2 | b >> 4);
        if chunk[2] == b'=' {
            if !last || chunk[3] != b'=' || b & 15 != 0 {
                return Err(Error::Invalid);
            }
        } else {
            let c = digit(chunk[2])?;
            decoded.push(b << 4 | c >> 2);
            if chunk[3] == b'=' {
                if !last || c & 3 != 0 {
                    return Err(Error::Invalid);
                }
            } else {
                decoded.push(c << 6 | digit(chunk[3])?);
            }
        }
    }
    Ok(decoded)
}

#[derive(Debug)]
pub struct Graphics {
    sources: BTreeMap<u32, Source>,
    placements: Vec<Placement>,
    pending: Option<Transfer>,
    legacy_pending: bool,
    generation: u64,
    next_id: u32,
    total_bytes: usize,
    image_limit: usize,
    total_limit: usize,
    image_count_limit: usize,
    placement_limit: usize,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
            placements: Vec::new(),
            pending: None,
            legacy_pending: false,
            generation: 0,
            next_id: 1,
            total_bytes: 0,
            image_limit: MAX_IMAGE_BYTES,
            total_limit: MAX_TOTAL_BYTES,
            image_count_limit: MAX_IMAGES,
            placement_limit: MAX_PLACEMENTS,
        }
    }
}

impl Graphics {
    pub fn forget_image(&mut self, id: u32) {
        let removed = self.sources.remove(&id);
        let count = self.placements.len();
        self.placements.retain(|p| p.image_id != id);
        if let Some(source) = &removed {
            self.total_bytes -= source.pixels.len();
        }
        if removed.is_some() || self.placements.len() != count {
            self.bump();
        }
    }

    pub fn abort_transfer(&mut self) {
        self.pending = None;
        self.legacy_pending = false;
    }
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }
    pub fn source(&self, id: u32) -> Option<&Source> {
        self.sources.get(&id)
    }
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn clear_all(&mut self) {
        self.sources.clear();
        self.placements.clear();
        self.pending = None;
        self.legacy_pending = false;
        self.total_bytes = 0;
        self.bump();
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Accept one complete APC including ESC_G and ST. The caller supplies the
    /// cursor at this command, particularly at the final transfer chunk.
    pub fn command(&mut self, apc: &[u8], cursor: (u16, u16), cell_px: (u16, u16)) -> Outcome {
        let (control, payload, error) = parse(apc);
        let legacy_start =
            control.number(b'U', 0) == Ok(1) && matches!(control.action(), Ok(b't' | b'T' | b'p'));
        // The compatibility path has no response channel. Never silently
        // accept a request whose client might be waiting for a reply.
        if error.is_none() && legacy_start && control.number(b'q', 0) != Ok(2) {
            self.abort_transfer();
            return control.reply(None, Some(Error::Unsupported));
        }
        if error.is_none() && (legacy_start || (self.legacy_pending && control.continuation())) {
            self.pending = None;
            self.legacy_pending = control.more().unwrap_or(false);
            return Outcome {
                legacy: true,
                ..Outcome::default()
            };
        }
        self.legacy_pending = false;
        if control.continuation()
            || (self.pending.is_some() && control.has(b'm') && !control.has(b'a'))
        {
            if let Some(mut transfer) = self.pending.take() {
                transfer.error = transfer.error.or(error);
                if !control.continuation() {
                    transfer.error = Some(Error::Invalid);
                }
                if let Some(quiet) = control.0.get(&b'q') {
                    transfer.control.0.insert(b'q', quiet.clone());
                    if control.number(b'q', 0).map_or(true, |q| q > 2) {
                        transfer.error = Some(Error::Invalid);
                    }
                }
                let more = control.more().unwrap_or(false);
                if control.more().is_err() {
                    transfer.error = Some(Error::Invalid);
                }
                transfer.append(payload, more);
                if more {
                    self.pending = Some(transfer);
                    return Outcome::default();
                }
                return self.finish(transfer, cursor, cell_px);
            }
            return control.reply(None, Some(Error::Invalid));
        }
        let action = match control.action() {
            Ok(action) => action,
            Err(error) => return control.reply(None, Some(error)),
        };
        // A delete explicitly aborts an unfinished image. Other interleaved
        // metadata is invalid, not silently spliced into the pending payload.
        if self.pending.take().is_some() && action != b'd' {
            return control.reply(None, Some(Error::Invalid));
        }
        if matches!(action, b't' | b'T' | b'q') {
            let mut transfer = Transfer::new(control, error, self.image_limit);
            let more = transfer.control.more().unwrap_or(false);
            transfer.append(payload, more);
            if more {
                self.pending = Some(transfer);
                return Outcome::default();
            }
            return self.finish(transfer, cursor, cell_px);
        }
        let result = (|| {
            if let Some(error) = error {
                return Err(error);
            }
            if !matches!(action, b'p' | b'd') {
                return Err(Error::Unsupported);
            }
            if !payload.is_empty() || control.has(b'm') {
                return Err(Error::Invalid);
            }
            match action {
                b'p' => {
                    control.validate(b"aiIpqxywhcrCXYUz")?;
                    let id = self.resolve(&control)?;
                    let source = self.sources.get(&id).ok_or(Error::Missing)?;
                    let (placement, moved) =
                        placement(&control, id, source.width, source.height, cursor, cell_px)?;
                    self.put(placement)?;
                    self.bump();
                    Ok((Some(id), moved))
                }
                b'd' => {
                    self.delete(&control)?;
                    Ok((None, None))
                }
                _ => Err(Error::Unsupported),
            }
        })();
        match result {
            Ok(_) if action == b'd' => Outcome::default(),
            Ok((id, cursor)) => Outcome {
                cursor,
                ..control.reply(id, None)
            },
            Err(error) => control.reply(None, Some(error)),
        }
    }

    fn fresh_id(&self) -> Result<u32> {
        let mut id = self.next_id.max(1);
        for _ in 0..=self.sources.len() {
            if !self.sources.contains_key(&id) {
                return Ok(id);
            }
            id = id.wrapping_add(1).max(1);
        }
        Err(Error::Quota)
    }

    fn resolve(&self, control: &Control) -> Result<u32> {
        if control.has(b'i') {
            return control.number(b'i', 0);
        }
        if control.has(b'I') {
            let number = control.number(b'I', 0)?;
            return self
                .sources
                .iter()
                .filter(|(_, s)| s.number == Some(number))
                .max_by_key(|(_, s)| s.revision)
                .map(|(&id, _)| id)
                .ok_or(Error::Missing);
        }
        Err(Error::Invalid)
    }

    fn finish(&mut self, transfer: Transfer, cursor: (u16, u16), cell_px: (u16, u16)) -> Outcome {
        let control = &transfer.control;
        let result = (|| {
            if let Some(error) = transfer.error {
                return Err(error);
            }
            if transfer.data.len() != transfer.expected {
                return Err(Error::Invalid);
            }
            let width = control.number(b's', 0)?;
            let height = control.number(b'v', 0)?;
            let action = control.action()?;
            let id = if control.has(b'i') {
                control.number(b'i', 0)?
            } else {
                self.fresh_id()?
            };
            let display = if action == b'T' {
                Some(placement(control, id, width, height, cursor, cell_px)?)
            } else {
                None
            };
            if action == b'q' {
                return Ok((id, None));
            }
            let previous_bytes = self.sources.get(&id).map_or(0, |s| s.pixels.len());
            if self.total_bytes - previous_bytes + transfer.data.len() > self.total_limit
                || (!self.sources.contains_key(&id) && self.sources.len() >= self.image_count_limit)
            {
                return Err(Error::Quota);
            }
            // Replacement deletes the previous image's placements atomically.
            let kept = self.placements.iter().filter(|p| p.image_id != id).count();
            if display.is_some() && kept >= self.placement_limit {
                return Err(Error::Quota);
            }
            self.total_bytes = self.total_bytes - previous_bytes + transfer.data.len();
            self.bump();
            self.sources.insert(
                id,
                Source {
                    width,
                    height,
                    format: control.number(b'f', 32)? as u8,
                    pixels: Arc::new(transfer.data),
                    revision: self.generation,
                    number: if control.has(b'I') {
                        Some(control.number(b'I', 0)?)
                    } else {
                        None
                    },
                },
            );
            self.next_id = id.wrapping_add(1).max(1);
            self.placements.retain(|p| p.image_id != id);
            let mut moved = None;
            if let Some((placement, movement)) = display {
                self.placements.push(placement);
                moved = movement;
            }
            Ok((id, moved))
        })();
        match result {
            Ok((id, cursor)) => Outcome {
                cursor,
                committed_image: (control.action() != Ok(b'q')).then_some(id),
                ..control.reply(Some(id), None)
            },
            Err(error) => control.reply(None, Some(error)),
        }
    }

    fn put(&mut self, placement: Placement) -> Result<()> {
        if placement.placement_id != 0 {
            if let Some(existing) = self.placements.iter_mut().find(|p| {
                p.image_id == placement.image_id && p.placement_id == placement.placement_id
            }) {
                *existing = placement;
                return Ok(());
            }
        }
        if self.placements.len() >= self.placement_limit {
            return Err(Error::Quota);
        }
        self.placements.push(placement);
        Ok(())
    }

    fn delete(&mut self, control: &Control) -> Result<()> {
        control.validate(b"adiIpqxy")?;
        let mode = control.letter(b'd', b'a')?;
        let lower = mode.to_ascii_lowercase();
        let specific = if matches!(lower, b'i' | b'n') {
            Some(self.resolve(control)?)
        } else {
            None
        };
        let low = control.number(b'x', 0)?;
        let high = control.number(b'y', u32::MAX)?;
        if lower == b'r' && (low == 0 || low > high) {
            return Err(Error::Invalid);
        }
        if !matches!(lower, b'a' | b'i' | b'n' | b'r') {
            return Err(Error::Unsupported);
        }
        let pid = control.number(b'p', 0)?;
        let selected = |id: u32| match lower {
            b'a' => true,
            b'i' | b'n' => Some(id) == specific,
            b'r' => id >= low && id <= high,
            _ => false,
        };
        self.placements
            .retain(|p| !(selected(p.image_id) && (pid == 0 || p.placement_id == pid)));
        if mode.is_ascii_uppercase() {
            self.sources
                .retain(|&id, _| !selected(id) || self.placements.iter().any(|p| p.image_id == id));
            self.total_bytes = self.sources.values().map(|s| s.pixels.len()).sum();
        }
        self.bump();
        Ok(())
    }
}

fn placement(
    control: &Control,
    image_id: u32,
    source_width: u32,
    source_height: u32,
    cursor: (u16, u16),
    cell_px: (u16, u16),
) -> Result<(Placement, Option<(u16, u16)>)> {
    if control.number(b'U', 0)? != 0 {
        return Err(Error::Unsupported);
    }
    let x = control.number(b'x', 0)?;
    let y = control.number(b'y', 0)?;
    if x >= source_width || y >= source_height {
        return Err(Error::Invalid);
    }
    let width = match control.number(b'w', 0)? {
        0 => source_width - x,
        n => n.min(source_width - x),
    };
    let height = match control.number(b'h', 0)? {
        0 => source_height - y,
        n => n.min(source_height - y),
    };
    let cols = control.number(b'c', 0)?;
    let rows = control.number(b'r', 0)?;
    let (cw, ch) = (cell_px.0 as u32, cell_px.1 as u32);
    let offset_x = control.number(b'X', 0)?;
    let offset_y = control.number(b'Y', 0)?;
    if (offset_x > 0 && offset_x >= cw) || (offset_y > 0 && offset_y >= ch) {
        return Err(Error::Invalid);
    }
    if (cols > 0 && cw == 0) || (rows > 0 && ch == 0) {
        return Err(Error::Invalid);
    }
    let pixel_width = cols
        .checked_mul(cw)
        .ok_or(Error::TooLarge)?
        .saturating_sub(offset_x);
    let pixel_height = rows
        .checked_mul(ch)
        .ok_or(Error::TooLarge)?
        .saturating_sub(offset_y);
    let (pixel_width, pixel_height) = match (cols, rows) {
        (0, 0) => (width, height),
        (_, 0) => (
            pixel_width,
            ((pixel_width as u64 * height as u64).div_ceil(width as u64)).min(u32::MAX as u64)
                as u32,
        ),
        (0, _) => (
            ((pixel_height as u64 * width as u64).div_ceil(height as u64)).min(u32::MAX as u64)
                as u32,
            pixel_height,
        ),
        _ => (pixel_width, pixel_height),
    };
    if pixel_width == 0
        || pixel_height == 0
        || pixel_width > MAX_DIMENSION
        || pixel_height > MAX_DIMENSION
        || pixel_width as u64 * pixel_height as u64 * 4 > MAX_IMAGE_BYTES as u64
    {
        return Err(Error::TooLarge);
    }
    let z_index = match control.0.get(&b'z') {
        Some(value) => value.parse::<i32>().map_err(|_| Error::Invalid)?,
        None => 0,
    };
    // Unicode-placeholder tiles cannot reproduce images behind arbitrary
    // terminal glyphs. Do not acknowledge unsupported text/image stacking.
    if z_index != 0 {
        return Err(Error::Unsupported);
    }
    let moved = match control.number(b'C', 0)? {
        1 => None,
        0 if cw > 0 && ch > 0 => Some((
            (cursor.0 as u32 + (pixel_height + offset_y).div_ceil(ch)).min(u16::MAX as u32) as u16,
            (cursor.1 as u32 + (pixel_width + offset_x).div_ceil(cw)).min(u16::MAX as u32) as u16,
        )),
        0 => return Err(Error::Invalid),
        _ => return Err(Error::Invalid),
    };
    Ok((
        Placement {
            image_id,
            placement_id: control.number(b'p', 0)?,
            row: cursor.0,
            col: cursor.1,
            x,
            y,
            width,
            height,
            pixel_width,
            pixel_height,
            offset_x,
            offset_y,
            z_index,
        },
        moved,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(g: &mut Graphics, controls: &str, payload: &str) -> Outcome {
        g.command(
            format!("\x1b_G{controls};{payload}\x1b\\").as_bytes(),
            (2, 3),
            (8, 16),
        )
    }

    fn ok(out: Outcome, id: u32) {
        assert_eq!(out.replies, format!("\x1b_Gi={id};OK\x1b\\").as_bytes());
        assert!(!out.unsupported);
        assert!(!out.legacy);
    }

    fn error(out: Outcome, id: u32, kind: &str) {
        let reply = String::from_utf8(out.replies).unwrap();
        assert!(
            reply.starts_with(&format!("\x1b_Gi={id};{kind}:")),
            "{reply:?}"
        );
        assert_eq!(out.unsupported, kind == "ENOTSUP");
        assert!(!out.legacy);
        assert!(out.cursor.is_none());
    }

    fn transmit(g: &mut Graphics, id: u32) {
        ok(command(g, &format!("a=t,i={id},f=24,s=1,v=1"), "AAAA"), id);
    }

    #[test]
    fn exact_tdf_probe_transfer_clear_redisplay_and_range_delete() {
        let mut g = Graphics::default();
        ok(command(&mut g, "i=31,s=1,v=1,a=q,t=d,f=24", "AAAA"), 31);
        assert!(g.source(31).is_none());
        assert_eq!(g.generation(), 0);
        error(
            command(&mut g, "a=q,i=4294967295,t=s,f=24,s=1,v=1", "L3RkZg=="),
            u32::MAX,
            "ENOTSUP",
        );
        ok(
            command(&mut g, "a=T,i=1,f=24,s=2,v=2,C=1", "AAAA/wAAAP8AAAD/"),
            1,
        );
        let revision = g.source(1).unwrap().revision;
        assert_eq!(
            &**g.source(1).unwrap().pixels,
            &[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]
        );
        assert_eq!((g.placements()[0].row, g.placements()[0].col), (2, 3));
        assert!(command(&mut g, "a=d,d=a", "").replies.is_empty());
        assert!(g.placements().is_empty());
        assert_eq!(g.source(1).unwrap().revision, revision);
        let shown = command(&mut g, "a=p,i=1,p=1,C=1,c=3,r=2", "");
        assert_eq!(shown.replies, b"\x1b_Gi=1,p=1;OK\x1b\\");
        assert!(shown.cursor.is_none());
        assert_eq!(
            (
                g.placements()[0].pixel_width,
                g.placements()[0].pixel_height
            ),
            (24, 32)
        );
        assert!(command(&mut g, "a=d,d=R,x=1,y=4294967295", "")
            .replies
            .is_empty());
        assert!(g.source(1).is_none());
        assert!(g.placements().is_empty());
        error(command(&mut g, "a=p,i=1,C=1", ""), 1, "ENOENT");
    }

    #[test]
    fn every_aligned_chunk_split_acks_only_final_and_uses_final_cursor() {
        let encoded = "AAAA/wAAAP8AAAD/";
        for split in (0..=encoded.len()).step_by(4) {
            let mut g = Graphics::default();
            let first = command(&mut g, "a=T,i=9,f=24,s=2,v=2,C=1,m=1", &encoded[..split]);
            assert!(first.replies.is_empty());
            assert!(g.source(9).is_none());
            assert!(g.placements().is_empty());
            assert_eq!(g.generation(), 0);
            let last = g.command(
                format!("\x1b_Gm=0;{}\x1b\\", &encoded[split..]).as_bytes(),
                (7, 11),
                (8, 16),
            );
            ok(last, 9);
            assert_eq!((g.placements()[0].row, g.placements()[0].col), (7, 11));
            assert_eq!(g.source(9).unwrap().pixels.len(), 12);
        }
        let mut g = Graphics::default();
        assert!(command(&mut g, "a=t,i=3,f=32,s=1,v=1,m=1", "AQID")
            .replies
            .is_empty());
        assert!(command(&mut g, "m=1", "").replies.is_empty());
        ok(command(&mut g, "m=0", "BA=="), 3);
        assert_eq!(g.source(3).unwrap().format, 32);
        assert_eq!(&**g.source(3).unwrap().pixels, &[1, 2, 3, 4]);
    }

    #[test]
    fn chunk_errors_remain_bounded_and_reply_only_at_final_with_original_id() {
        let mut g = Graphics::default();
        assert!(command(&mut g, "a=t,i=42,f=24,s=1,v=1,m=1", "!!!!")
            .replies
            .is_empty());
        assert!(command(&mut g, "m=1", &"A".repeat(MAX_CHUNK))
            .replies
            .is_empty());
        assert!(g.pending.as_ref().unwrap().data.is_empty());
        error(command(&mut g, "m=0", "AAAA"), 42, "EINVAL");
        assert!(g.sources.is_empty());
        assert!(g.pending.is_none());
        assert!(command(&mut g, "a=q,i=42,t=s,s=1,v=1,m=1", "AAAA")
            .replies
            .is_empty());
        error(command(&mut g, "m=0", ""), 42, "ENOTSUP");
        assert!(g.sources.is_empty());
        command(&mut g, "a=t,i=42,f=24,s=2,v=1,m=1", "AAAA");
        error(command(&mut g, "m=0,s=2", "AAAA"), 42, "EINVAL");
        assert!(g.source(42).is_none());
    }

    #[test]
    fn rejects_invalid_sizes_formats_media_and_payloads_truthfully() {
        let cases = [
            ("a=q,i=3,f=24,s=0,v=1", "AAAA", "EINVAL"),
            ("a=q,i=3,f=24,s=1,v=0", "AAAA", "EINVAL"),
            ("a=q,i=3,f=24,s=4294967296,v=1", "AAAA", "EINVAL"),
            ("a=q,i=3,f=24,s=4294967295,v=4294967295", "AAAA", "E2BIG"),
            ("a=q,i=3,f=24,s=16384,v=16384", "AAAA", "E2BIG"),
            ("a=q,i=3,f=100,s=1,v=1", "AAAA", "ENOTSUP"),
            ("a=q,i=3,f=24,s=1,v=1,t=f", "L2V0Yy9wYXNzd2Q=", "ENOTSUP"),
            ("a=q,i=3,f=24,s=1,v=1,t=t", "L3RtcC9pbWc=", "ENOTSUP"),
            ("a=q,i=3,f=24,s=1,v=1,o=z", "AAAA", "ENOTSUP"),
            ("a=q,i=3,f=24,s=1,v=1", "AA==", "EINVAL"),
            ("a=q,i=3,f=24,s=1,v=1", "AAAAAAAA", "EINVAL"),
            ("a=q,i=3,f=24,s=1,v=1", "AA_A", "EINVAL"),
            ("a=q,i=3,f=24,s=1,v=1", "AAA", "EINVAL"),
            ("a=q,i=3,f=24,s=1,v=1", "AAAA\n", "EINVAL"),
            ("a=t,i=3,f=32,s=1,v=1", "AQIDBB==", "EINVAL"),
            ("a=f,i=3", "AAAA", "ENOTSUP"),
            ("a=t,i=3,f=24,s=1,v=1,m=2", "AAAA", "EINVAL"),
            ("a=t,i=3,f=24,s=1,v=1,s=2", "AAAA", "EINVAL"),
            ("a=t,i=3,f=24,s=1,v=1,P=1", "AAAA", "ENOTSUP"),
        ];
        for (controls, payload, kind) in cases {
            let mut g = Graphics::default();
            error(command(&mut g, controls, payload), 3, kind);
            assert!(g.sources.is_empty(), "{controls}");
            assert!(g.placements().is_empty());
            assert_eq!(g.generation(), 0);
        }
    }

    #[test]
    fn strict_base64_padding_and_all_byte_values() {
        assert_eq!(decode(b"AQIDBA==", true), Ok(vec![1, 2, 3, 4]));
        assert_eq!(decode(b"//79", true), Ok(vec![255, 254, 253]));
        for invalid in [
            b"A===".as_slice(),
            b"=AAA",
            b"AA=A",
            b"AA==AAAA",
            b"AB==",
            b"AAB=",
            b"AA-_",
            b"AAA ",
        ] {
            assert_eq!(decode(invalid, true), Err(Error::Invalid), "{invalid:?}");
        }
        assert_eq!(decode(b"AA==", false), Err(Error::Invalid));
        assert_eq!(decode(b"AAA=", false), Err(Error::Invalid));
        assert_eq!(decode(b"", true), Ok(vec![]));
        let mut g = Graphics::default();
        error(
            command(&mut g, "a=t,i=3,f=24,s=1,v=1", &"A".repeat(MAX_CHUNK + 4)),
            3,
            "E2BIG",
        );
    }

    #[test]
    fn image_and_total_budgets_count_limits_and_atomic_failed_replacement() {
        let mut g = Graphics {
            image_limit: 6,
            total_limit: 6,
            image_count_limit: 2,
            ..Graphics::default()
        };
        transmit(&mut g, 1);
        transmit(&mut g, 2);
        let old = g.source(1).unwrap().pixels.clone();
        let revision = g.source(1).unwrap().revision;
        error(command(&mut g, "a=t,i=3,f=24,s=1,v=1", "AAAA"), 3, "ENOSPC");
        error(
            command(&mut g, "a=t,i=1,f=24,s=2,v=1", "AAAAAAAA"),
            1,
            "ENOSPC",
        );
        error(
            command(&mut g, "a=t,i=1,f=24,s=3,v=1", "AAAAAAAAAAAA"),
            1,
            "E2BIG",
        );
        assert_eq!(g.source(1).unwrap().revision, revision);
        assert!(Arc::ptr_eq(&g.source(1).unwrap().pixels, &old));
        assert_eq!(g.total_bytes, 6);
        command(&mut g, "a=d,d=I,i=2", "");
        ok(command(&mut g, "a=t,i=1,f=24,s=2,v=1", "AAAAAAAA"), 1);
        assert_eq!(g.total_bytes, 6);
        assert_ne!(g.source(1).unwrap().revision, revision);
        assert_eq!(&**old, &[0, 0, 0]);
        let mut count_limited = Graphics {
            image_count_limit: 1,
            ..Graphics::default()
        };
        transmit(&mut count_limited, 1);
        error(
            command(&mut count_limited, "a=t,i=2,f=24,s=1,v=1", "AAAA"),
            2,
            "ENOSPC",
        );
    }

    #[test]
    fn placement_limits_upserts_and_retransmission_remove_old_placements() {
        let mut g = Graphics {
            placement_limit: 2,
            ..Graphics::default()
        };
        transmit(&mut g, 1);
        ok(command(&mut g, "a=p,i=1,C=1", ""), 1);
        ok(command(&mut g, "a=p,i=1,C=1", ""), 1);
        error(command(&mut g, "a=p,i=1,C=1", ""), 1, "ENOSPC");
        assert_eq!(g.placements().len(), 2);
        transmit(&mut g, 1);
        assert!(g.placements().is_empty());
        command(&mut g, "a=p,i=1,p=9,C=1,c=1", "");
        command(&mut g, "a=p,i=1,p=9,C=1,c=2", "");
        assert_eq!(g.placements().len(), 1);
        assert_eq!(g.placements()[0].pixel_width, 16);
        let revision = g.source(1).unwrap().revision;
        error(
            command(&mut g, "a=T,i=1,f=24,s=1,v=1,C=1,x=2", "AAAA"),
            1,
            "EINVAL",
        );
        assert_eq!(g.source(1).unwrap().revision, revision);
        assert_eq!(g.placements().len(), 1);
    }

    #[test]
    fn crop_intersection_aspect_ratio_offsets_and_cursor_policy() {
        let c = |raw: &str| parse(format!("\x1b_G{raw}\x1b\\").as_bytes()).0;
        let (p, moved) = placement(
            &c("C=1,x=10,y=5,w=300,h=100,c=10,X=4"),
            1,
            100,
            50,
            (7, 11),
            (8, 16),
        )
        .unwrap();
        assert_eq!((p.x, p.y, p.width, p.height), (10, 5, 90, 45));
        assert_eq!((p.pixel_width, p.pixel_height), (76, 38));
        assert_eq!((p.row, p.col, p.offset_x, p.offset_y), (7, 11, 4, 0));
        assert_eq!(moved, None);
        let (p, moved) = placement(&c("r=2"), 1, 100, 50, (7, 11), (8, 16)).unwrap();
        assert_eq!((p.pixel_width, p.pixel_height), (64, 32));
        assert_eq!(moved, Some((9, 19)));
        let (p, _) = placement(&c("c=10,r=3,C=1,Y=2,z=0"), 1, 100, 50, (0, 0), (8, 16)).unwrap();
        assert_eq!((p.pixel_width, p.pixel_height, p.z_index), (80, 46, 0));
        assert_eq!(
            placement(&c("C=1,z=-1"), 1, 100, 50, (0, 0), (8, 16)),
            Err(Error::Unsupported)
        );
        for raw in [
            "x=100,C=1",
            "y=50,C=1",
            "C=2",
            "X=8,C=1",
            "Y=16,C=1",
            "c=4294967295,C=1",
            "z=2147483648,C=1",
        ] {
            assert!(
                placement(&c(raw), 1, 100, 50, (0, 0), (8, 16)).is_err(),
                "{raw}"
            );
        }
        assert!(placement(&c("c=1,C=1"), 1, 100, 50, (0, 0), (0, 0)).is_err());
        assert!(placement(&c("C=1"), 1, 100, 50, (0, 0), (0, 0)).is_ok());
    }

    #[test]
    fn quiet_modes_and_final_chunk_quiet_override() {
        let mut g = Graphics::default();
        assert!(command(&mut g, "a=t,i=1,q=1,f=24,s=1,v=1", "AAAA")
            .replies
            .is_empty());
        assert!(g.source(1).is_some());
        error(command(&mut g, "a=p,i=8,q=1,C=1", ""), 8, "ENOENT");
        assert!(command(&mut g, "a=p,i=8,q=2,C=1", "").replies.is_empty());
        assert!(command(&mut g, "a=t,i=2,q=0,f=24,s=1,v=1,m=1", "AAAA")
            .replies
            .is_empty());
        assert!(command(&mut g, "m=0,q=2", "").replies.is_empty());
        assert!(g.source(2).is_some());
        error(command(&mut g, "a=p,i=8,q=3,C=1", ""), 8, "EINVAL");
    }

    #[test]
    fn deletion_is_pane_local_and_lowercase_keeps_sources() {
        let mut left = Graphics::default();
        let mut right = Graphics::default();
        for g in [&mut left, &mut right] {
            for id in [1, 2, 3] {
                transmit(g, id);
            }
            command(g, "a=p,i=2,p=7,C=1", "");
            command(g, "a=p,i=2,p=8,C=1", "");
        }
        command(&mut left, "a=d,d=I,i=2,p=7", "");
        assert!(left.source(2).is_some());
        assert_eq!(left.placements().len(), 1);
        command(&mut left, "a=d,d=r,x=2,y=2", "");
        assert!(left.source(2).is_some());
        assert!(left.placements().is_empty());
        command(&mut left, "a=d,d=R,x=2,y=3", "");
        assert!(left.source(1).is_some());
        assert!(left.source(2).is_none());
        assert!(left.source(3).is_none());
        assert_eq!(right.sources.len(), 3);
        assert_eq!(right.placements().len(), 2);
        assert_eq!(left.total_bytes, 3);
        let generation = left.generation();
        left.clear_all();
        assert!(left.sources.is_empty());
        assert!(left.placements().is_empty());
        assert_eq!(left.total_bytes, 0);
        assert!(left.generation() > generation);
    }

    #[test]
    fn delete_aborts_pending_and_interleaving_cannot_splice_images() {
        let mut g = Graphics::default();
        command(&mut g, "a=t,i=7,f=24,s=2,v=1,m=1", "AAAA");
        command(&mut g, "a=d,d=a", "");
        assert!(g.pending.is_none());
        command(&mut g, "m=0", "AAAA");
        assert!(g.source(7).is_none());
        command(&mut g, "a=t,i=7,f=24,s=2,v=1,m=1", "AAAA");
        error(command(&mut g, "a=t,i=8,f=24,s=1,v=1", "AAAA"), 8, "EINVAL");
        assert!(g.sources.is_empty());
        transmit(&mut g, 8);
        assert!(g.source(8).is_some());
    }

    #[test]
    fn legacy_virtual_placements_and_all_continuations_are_routed_without_local_acks() {
        let mut g = Graphics::default();
        for (control, payload) in [
            ("a=T,U=1,i=5,f=100,m=1,q=2", "aGVs"),
            ("m=1,q=2", "bG8="),
            ("m=0", ""),
            ("a=p,U=1,i=5,c=2,r=2,q=2", ""),
        ] {
            let out = command(&mut g, control, payload);
            assert!(out.legacy);
            assert!(!out.unsupported);
            assert!(out.replies.is_empty());
        }
        assert!(g.sources.is_empty());
        assert!(g.placements().is_empty());
        assert_eq!(g.generation(), 0);
        assert!(!command(&mut g, "m=0", "").legacy);
        transmit(&mut g, 5);
        error(
            command(&mut g, "a=q,U=1,i=5,f=24,s=1,v=1", "AAAA"),
            5,
            "ENOTSUP",
        );
    }

    #[test]
    fn nonquiet_virtual_starts_are_rejected_without_hanging_clients() {
        let mut g = Graphics::default();
        for action in ['t', 'T', 'p'] {
            for quiet in ["", ",q=0", ",q=1"] {
                let out = command(
                    &mut g,
                    &format!("a={action},U=1,i=5,f=24,s=1,v=1{quiet}"),
                    "AAAA",
                );
                assert!(!out.legacy);
                error(out, 5, "ENOTSUP");
                assert!(!g.legacy_pending);
            }
        }
        assert!(g.sources.is_empty());
        assert!(g.placements().is_empty());
        assert!(command(&mut g, "a=T,U=1,i=5,f=100,m=1,q=2", "AAAA").legacy);
        // Quiet compatibility is chosen at the start, not switched by later chunks.
        for controls in ["m=1,q=0", "m=0,q=1"] {
            let out = command(&mut g, controls, "AAAA");
            assert!(out.legacy);
            assert!(out.replies.is_empty());
        }
        assert!(command(&mut g, "a=T,U=1,i=5,f=100,m=1,q=2", "AAAA").legacy);
        error(command(&mut g, "a=p,U=1,i=5,q=1", ""), 5, "ENOTSUP");
        assert!(!command(&mut g, "m=0", "").legacy);
    }

    #[test]
    fn source_commit_markers_exclude_queries_placements_and_failed_transfers() {
        let mut g = Graphics::default();
        let out = command(&mut g, "a=T,i=7,f=24,s=1,v=1,C=1", "AQID");
        assert_eq!(out.committed_image, Some(7));
        for (header, data) in [
            ("a=q,i=7,f=24,s=1,v=1", "BAUG"),
            ("a=p,i=7,C=1", ""),
            ("a=t,i=7,f=24,s=1,v=1", "AQI="),
            ("a=d,d=a", ""),
        ] {
            assert_eq!(command(&mut g, header, data).committed_image, None);
        }
        assert_eq!(
            command(&mut g, "a=t,i=8,f=24,s=1,v=1", "BAUG").committed_image,
            Some(8)
        );
        let generation = g.generation();
        g.forget_image(7);
        assert!(g.source(7).is_none());
        assert!(g.source(8).is_some());
        assert_eq!(g.total_bytes, 3);
        assert!(g.generation() > generation);
        let generation = g.generation();
        g.forget_image(7);
        assert_eq!(g.generation(), generation);
    }

    #[test]
    fn query_same_id_never_replaces_source_or_changes_placements() {
        let mut g = Graphics::default();
        ok(command(&mut g, "a=T,i=31,f=24,s=1,v=1,C=1", "AAAA"), 31);
        let old = g.source(31).unwrap().pixels.clone();
        let revision = g.source(31).unwrap().revision;
        let generation = g.generation();
        let placements = g.placements().to_vec();
        ok(command(&mut g, "a=q,i=31,f=24,s=1,v=1", "////"), 31);
        error(
            command(&mut g, "a=q,i=31,f=24,s=1,v=1", "!!!!"),
            31,
            "EINVAL",
        );
        assert!(Arc::ptr_eq(&old, &g.source(31).unwrap().pixels));
        assert_eq!(g.source(31).unwrap().revision, revision);
        assert_eq!(g.placements(), placements);
        assert_eq!(g.generation(), generation);
        assert!(command(&mut g, "a=d,d=I,i=31", "").replies.is_empty());
    }

    #[test]
    fn numbered_images_allocate_distinct_ids_and_resolve_newest() {
        let mut g = Graphics::default();
        let first = command(&mut g, "a=t,I=13,f=24,s=1,v=1", "AAAA");
        assert_eq!(first.replies, b"\x1b_Gi=1,I=13;OK\x1b\\");
        let second = command(&mut g, "a=t,I=13,f=24,s=1,v=1", "////");
        assert_eq!(second.replies, b"\x1b_Gi=2,I=13;OK\x1b\\");
        let displayed = command(&mut g, "a=p,I=13,C=1", "");
        assert_eq!(displayed.replies, b"\x1b_Gi=2,I=13;OK\x1b\\");
        assert_eq!(g.placements()[0].image_id, 2);
        let invalid = command(&mut g, "a=t,i=5,I=13,f=24,s=1,v=1", "AAAA");
        assert!(String::from_utf8(invalid.replies)
            .unwrap()
            .contains("EINVAL:"));
        assert!(g.source(5).is_none());
        command(&mut g, "a=d,d=N,I=13", "");
        assert!(g.source(2).is_none());
        assert!(g.source(1).is_some());
        g.next_id = u32::MAX;
        let last = command(&mut g, "a=t,I=14,f=24,s=1,v=1", "AAAA");
        assert_eq!(last.replies, b"\x1b_Gi=4294967295,I=14;OK\x1b\\");
        let wrapped = command(&mut g, "a=t,I=15,f=24,s=1,v=1", "AAAA");
        assert_eq!(wrapped.replies, b"\x1b_Gi=2,I=15;OK\x1b\\");
    }
}
