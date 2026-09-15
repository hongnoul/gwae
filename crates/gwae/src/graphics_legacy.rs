//! Bounded compatibility for quiet Kitty Unicode-placeholder producers.
//!
//! Child commands are never replayed. Direct pixels and a sanitized, validated
//! PNG are re-encoded into canonical quiet commands in a process-wide host ID
//! namespace. Retained command allocations are capped at 128 MiB per pane,
//! with one additional <=32 MiB input transfer and bounded validation scratch.
use std::cell::Cell as StateCell;
use std::collections::BTreeMap;

use crate::graphics_diacritics::DIACRITICS;
use crate::graphics_host::{allocate, id_color, Host, PLACEHOLDER};
use gwae_term::{CColor, Cell, NO_COMBINING};

const MAX_INPUT: usize = 32 * 1024 * 1024;
const MAX_RETAINED: usize = 128 * 1024 * 1024;
const MAX_IMAGES: usize = 64;
const MAX_PLACEMENTS: usize = 256;
const MAX_AXIS: u32 = 16384;
const MAX_VIRTUAL_AXIS: u32 = 256;

#[derive(Default)]
struct Header(BTreeMap<u8, String>);
impl Header {
    fn number(&self, key: u8, default: u32) -> Option<u32> {
        match self.0.get(&key) {
            None => Some(default),
            Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => v.parse().ok(),
            _ => None,
        }
    }
    fn letter(&self, key: u8, default: u8) -> Option<u8> {
        match self.0.get(&key) {
            None => Some(default),
            Some(v) if v.len() == 1 => Some(v.as_bytes()[0]),
            _ => None,
        }
    }
    fn allowed(&self, keys: &[u8]) -> bool {
        self.0.keys().all(|k| keys.contains(k))
    }
    fn continuation(&self) -> bool {
        self.0.contains_key(&b'm') && self.allowed(b"mq")
    }
    fn more(&self) -> Option<bool> {
        match self.number(b'm', 0)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}
fn parse(apc: &[u8]) -> Option<(Header, &[u8])> {
    let body = apc.strip_prefix(b"\x1b_G")?.strip_suffix(b"\x1b\\")?;
    let split = body.iter().position(|&b| b == b';').unwrap_or(body.len());
    if split == 0 || split > 1024 {
        return None;
    }
    let mut h = Header::default();
    for pair in body[..split].split(|&b| b == b',') {
        if pair.len() < 3
            || pair[1] != b'='
            || !pair[0].is_ascii_alphabetic()
            || !pair[2..].iter().all(u8::is_ascii_alphanumeric)
        {
            return None;
        }
        if h.0
            .insert(pair[0], String::from_utf8(pair[2..].to_vec()).ok()?)
            .is_some()
        {
            return None;
        }
    }
    if h.number(b'q', 0)? > 2 {
        return None;
    }
    Some((h, body.get(split + 1..).unwrap_or_default()))
}

fn decode(chunk: &[u8], more: bool) -> Option<Vec<u8>> {
    if chunk.len() > 4096 || chunk.len() % 4 != 0 {
        return None;
    }
    fn digit(b: u8) -> Option<u8> {
        Some(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(chunk.len() / 4 * 3);
    for (i, q) in chunk.chunks_exact(4).enumerate() {
        let a = digit(q[0])?;
        let b = digit(q[1])?;
        out.push(a << 2 | b >> 4);
        if q[2] == b'=' {
            if q[3] != b'=' || more || (i + 1) * 4 != chunk.len() || b & 15 != 0 {
                return None;
            }
        } else {
            let c = digit(q[2])?;
            out.push(b << 4 | c >> 2);
            if q[3] == b'=' {
                if more || (i + 1) * 4 != chunk.len() || c & 3 != 0 {
                    return None;
                }
            } else {
                out.push(c << 6 | digit(q[3])?);
            }
        }
    }
    Some(out)
}
fn encoded(data: &[u8]) -> Vec<u8> {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for p in data.chunks(3) {
        let a = p[0] as usize;
        let b = p.get(1).copied().unwrap_or(0) as usize;
        let c = p.get(2).copied().unwrap_or(0) as usize;
        out.extend_from_slice(&[
            ABC[a >> 2],
            ABC[((a & 3) << 4) | (b >> 4)],
            if p.len() > 1 {
                ABC[((b & 15) << 2) | (c >> 6)]
            } else {
                b'='
            },
            if p.len() > 2 { ABC[c & 63] } else { b'=' },
        ]);
    }
    out
}
fn footprint(w: u32, h: u32, channels: usize) -> Option<usize> {
    if w == 0 || h == 0 || w > MAX_AXIS || h > MAX_AXIS {
        return None;
    }
    let bytes = (w as usize)
        .checked_mul(h as usize)?
        .checked_mul(channels)?;
    (bytes <= MAX_INPUT).then_some(bytes)
}

#[derive(Clone)]
struct Placement {
    id: u32,
    cols: u32,
    rows: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
impl Placement {
    fn new(h: &Header, width: u32, height: u32) -> Option<Self> {
        let cols = h.number(b'c', 0)?;
        let rows = h.number(b'r', 0)?;
        if cols > MAX_VIRTUAL_AXIS
            || rows > MAX_VIRTUAL_AXIS
            || h.number(b'C', 0)? > 1
            || h.number(b'z', 0)? != 0
        {
            return None;
        }
        let x = h.number(b'x', 0)?;
        let y = h.number(b'y', 0)?;
        if x >= width || y >= height {
            return None;
        }
        let crop_w = h.number(b'w', 0)?;
        let crop_h = h.number(b'h', 0)?;
        Some(Self {
            id: h.number(b'p', 0)?,
            cols,
            rows,
            x,
            y,
            width: if crop_w == 0 {
                width - x
            } else {
                crop_w.min(width - x)
            },
            height: if crop_h == 0 {
                height - y
            } else {
                crop_h.min(height - y)
            },
        })
    }
    fn append(&self, out: &mut Vec<u8>, host: u32) {
        out.extend_from_slice(
            format!(
                "\x1b_Ga=p,U=1,i={host},p={},c={},r={},x={},y={},w={},h={},C=1,q=2\x1b\\",
                self.id, self.cols, self.rows, self.x, self.y, self.width, self.height
            )
            .as_bytes(),
        );
    }
}
struct Image {
    host_id: u32,
    revision: u64,
    width: u32,
    height: u32,
    footprint: usize,
    commands: Vec<u8>,
    source_end: usize,
    placements: Vec<Placement>,
}
struct Transfer {
    header: Header,
    data: Vec<u8>,
    limit: usize,
    format: u32,
    width: u32,
    height: u32,
}
impl Transfer {
    fn new(header: Header) -> Option<Self> {
        if !header.allowed(b"aUifsvtqpcrxywhCmz")
            || header.number(b'U', 0)? != 1
            || header.letter(b't', b'd')? != b'd'
            || header.number(b'i', 0)? == 0
            || !matches!(header.letter(b'a', b't')?, b't' | b'T')
        {
            return None;
        }
        header.more()?;
        let format = header.number(b'f', 32)?;
        let width = header.number(b's', 0)?;
        let height = header.number(b'v', 0)?;
        let limit = match format {
            24 | 32 => {
                footprint(width, height, 4)?;
                footprint(width, height, (format / 8) as usize)?
            }
            100 => MAX_INPUT,
            _ => return None,
        };
        let mut data = Vec::new();
        data.try_reserve_exact(limit).ok()?;
        Some(Self {
            header,
            data,
            limit,
            format,
            width,
            height,
        })
    }
    fn append(&mut self, payload: &[u8], more: bool) -> Option<()> {
        let bytes = decode(payload, more)?;
        if self.data.len().checked_add(bytes.len())? > self.limit {
            return None;
        }
        self.data.extend_from_slice(&bytes);
        Some(())
    }
}

#[derive(Clone, Copy)]
struct Address {
    row: usize,
    col: usize,
    high: usize,
    fg: CColor,
    underline: CColor,
}
#[derive(Default)]
pub struct Legacy {
    images: BTreeMap<u32, Image>,
    pending: Option<Transfer>,
    bytes: usize,
    revision: u64,
    committed_image: Option<u32>,
    previous: StateCell<Option<Address>>,
}
impl Legacy {
    /// Commit event for the last accepted APC, consumed by the pane router.
    pub fn take_committed_image(&mut self) -> Option<u32> {
        self.committed_image.take()
    }
    /// Bumped on every image commit, placement update, or delete. Paired with
    /// `Graphics::generation` as the pane's image-change token.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn forget_image(&mut self, id: u32) {
        if let Some(image) = self.images.remove(&id) {
            self.bytes -= image.commands.capacity();
        }
        if self.committed_image == Some(id) {
            self.committed_image = None;
        }
    }
    pub fn abort_transfer(&mut self) {
        self.pending = None;
    }
    pub fn begin_row(&self) {
        self.previous.set(None);
    }
    /// Resolve a hidden prefix cell without uploading it, before horizontal clipping.
    pub fn observe(&self, cell: Cell) {
        let _ = self.address(cell);
    }
    fn bump(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.revision
    }
    fn count(&self) -> usize {
        self.images.values().map(|i| i.placements.len()).sum()
    }
    pub fn accept(&mut self, apc: &[u8]) {
        self.committed_image = None;
        let Some((header, payload)) = parse(apc) else {
            self.abort_transfer();
            return;
        };
        let Some(more) = header.more() else {
            self.abort_transfer();
            return;
        };
        if header.continuation() {
            let Some(mut transfer) = self.pending.take() else {
                return;
            };
            if transfer.append(payload, more).is_none() {
                return;
            }
            if more {
                self.pending = Some(transfer)
            } else {
                self.finish(transfer)
            }
            return;
        }
        self.abort_transfer();
        if header.letter(b'a', b't') == Some(b'p') {
            if more
                || header.0.contains_key(&b'm')
                || !payload.is_empty()
                || !header.allowed(b"aUipqcrxywhCz")
                || header.number(b'U', 0) != Some(1)
            {
                return;
            }
            let Some(id) = header.number(b'i', 0) else {
                return;
            };
            let Some(image) = self.images.get(&id) else {
                return;
            };
            let Some(p) = Placement::new(&header, image.width, image.height) else {
                return;
            };
            let mut placements = image.placements.clone();
            if let Some(old) = placements
                .iter_mut()
                .find(|old| p.id != 0 && old.id == p.id)
            {
                *old = p;
            } else {
                if self.count() >= MAX_PLACEMENTS {
                    return;
                }
                placements.push(p);
            }
            let Some(commands) = rebuild(image, &placements) else {
                return;
            };
            if self.bytes - image.commands.capacity() + commands.capacity() > MAX_RETAINED {
                return;
            }
            let revision = self.bump();
            let image = self.images.get_mut(&id).unwrap();
            self.bytes = self.bytes - image.commands.capacity() + commands.capacity();
            image.commands = commands;
            image.placements = placements;
            image.revision = revision;
            return;
        }
        let Some(mut transfer) = Transfer::new(header) else {
            return;
        };
        if transfer.append(payload, more).is_none() {
            return;
        }
        if more {
            self.pending = Some(transfer)
        } else {
            self.finish(transfer)
        }
    }
    fn finish(&mut self, mut t: Transfer) {
        let (width, height, host_bytes) = if t.format == 100 {
            let Some((data, w, h, bytes)) = sanitize_png(&t.data) else {
                return;
            };
            if (t.width != 0 && t.width != w) || (t.height != 0 && t.height != h) {
                return;
            }
            t.data = data;
            (w, h, bytes)
        } else {
            if t.data.len() != t.limit {
                return;
            }
            (t.width, t.height, footprint(t.width, t.height, 4).unwrap())
        };
        let id = t.header.number(b'i', 0).unwrap();
        if !self.images.contains_key(&id) && self.images.len() >= MAX_IMAGES {
            return;
        }
        let placements = if t.header.letter(b'a', b't') == Some(b'T') {
            let Some(p) = Placement::new(&t.header, width, height) else {
                return;
            };
            vec![p]
        } else {
            Vec::new()
        };
        let old_count = self.images.get(&id).map_or(0, |i| i.placements.len());
        if self.count() - old_count + placements.len() > MAX_PLACEMENTS {
            return;
        }
        let host_id = allocate();
        let Some(mut commands) = transmit(host_id, t.format, width, height, &t.data) else {
            return;
        };
        let source_end = commands.len();
        for p in &placements {
            p.append(&mut commands, host_id);
        }
        commands.shrink_to_fit();
        let old_bytes = self.images.get(&id).map_or(0, |i| i.commands.capacity());
        if self.bytes - old_bytes + commands.capacity() > MAX_RETAINED {
            return;
        }
        self.bytes = self.bytes - old_bytes + commands.capacity();
        let revision = self.bump();
        self.images.insert(
            id,
            Image {
                host_id,
                revision,
                width,
                height,
                footprint: host_bytes,
                commands,
                source_end,
                placements,
            },
        );
        self.committed_image = Some(id);
    }
    /// Called for every APC. Deletes always abort assembly, including unsupported deletes.
    pub fn delete_command(&mut self, apc: &[u8]) {
        let Some((h, payload)) = parse(apc) else {
            self.abort_transfer();
            return;
        };
        if h.letter(b'a', b't') != Some(b'd') {
            if !h.continuation() && (h.number(b'U', 0) != Some(1) || h.number(b'q', 0) != Some(2)) {
                self.abort_transfer();
            }
            return;
        }
        self.abort_transfer();
        if !payload.is_empty() || !h.allowed(b"adipxyq") {
            return;
        }
        let Some(mode) = h.letter(b'd', b'a') else {
            return;
        };
        // Virtual placements have no physical location: d=a/A never affects them.
        if !matches!(mode, b'i' | b'I' | b'r' | b'R') {
            return;
        }
        let Some(id) = h.number(b'i', 0) else { return };
        let Some(pid) = h.number(b'p', 0) else { return };
        let Some(low) = h.number(b'x', 0) else { return };
        let Some(high) = h.number(b'y', u32::MAX) else {
            return;
        };
        if (matches!(mode, b'i' | b'I') && id == 0)
            || (matches!(mode, b'r' | b'R') && (low == 0 || low > high))
        {
            return;
        }
        let free = mode.is_ascii_uppercase();
        let revision = self.bump();
        self.images.retain(|&key, image| {
            let selected = if matches!(mode, b'i' | b'I') {
                key == id
            } else {
                key >= low && key <= high
            };
            if !selected {
                return true;
            }
            image.placements.retain(|p| pid != 0 && p.id != pid);
            if free && image.placements.is_empty() {
                return false;
            }
            image.commands.truncate(image.source_end);
            for p in &image.placements {
                p.append(&mut image.commands, image.host_id);
            }
            image.revision = revision;
            true
        });
        self.bytes = self.images.values().map(|i| i.commands.capacity()).sum();
    }
    fn address(&self, cell: Cell) -> Option<Address> {
        if cell.ch != PLACEHOLDER {
            self.previous.set(None);
            return None;
        }
        let count = cell.combining.iter().take_while(|&&c| c != '\0').count();
        let index = |c| DIACRITICS.iter().position(|&d| d == c);
        let previous = self
            .previous
            .get()
            .filter(|p| p.fg == cell.style.fg && p.underline == cell.style.underline_color);
        let result = (|| {
            let row = if count > 0 {
                index(cell.combining[0])?
            } else {
                previous.map_or(0, |p| p.row)
            };
            let adjacent = previous.filter(|p| p.row == row);
            let col = if count > 1 {
                index(cell.combining[1])?
            } else {
                adjacent.map_or(Some(0), |p| p.col.checked_add(1))?
            };
            let high = if count > 2 {
                index(cell.combining[2])?
            } else {
                adjacent
                    .filter(|p| p.col.checked_add(1) == Some(col))
                    .map_or(0, |p| p.high)
            };
            if row >= DIACRITICS.len() || col >= DIACRITICS.len() || high >= 256 {
                return None;
            }
            Some(Address {
                row,
                col,
                high,
                fg: cell.style.fg,
                underline: cell.style.underline_color,
            })
        })();
        self.previous.set(result);
        result
    }
    pub fn cell(&self, mut cell: Cell, host: &mut Host) -> Cell {
        if cell.ch != PLACEHOLDER {
            self.previous.set(None);
            return cell;
        }
        let Some(address) = self.address(cell) else {
            return blank(cell);
        };
        let id = color_number(address.fg) | (address.high as u32) << 24;
        let Some(image) = self.images.get(&id) else {
            return blank(cell);
        };
        let pid = color_number(cell.style.underline_color);
        if !image.placements.iter().any(|p| pid == 0 || p.id == pid)
            || !host.show_legacy(
                image.host_id,
                image.revision,
                &image.commands,
                image.footprint,
            )
        {
            return blank(cell);
        }
        cell.style.fg = id_color(image.host_id);
        cell.combining = NO_COMBINING;
        cell.combining[0] = DIACRITICS[address.row];
        cell.combining[1] = DIACRITICS[address.col];
        cell.combining[2] = DIACRITICS[(image.host_id >> 24) as usize];
        cell
    }
}
fn color_number(color: CColor) -> u32 {
    match color {
        CColor::Default => 0,
        CColor::Idx(n) => n as u32,
        CColor::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
    }
}
fn blank(mut cell: Cell) -> Cell {
    cell.ch = ' ';
    cell.combining = NO_COMBINING;
    cell.width = 1;
    cell
}
fn rebuild(image: &Image, placements: &[Placement]) -> Option<Vec<u8>> {
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(image.source_end + placements.len() * 192)
        .ok()?;
    commands.extend_from_slice(&image.commands[..image.source_end]);
    for p in placements {
        p.append(&mut commands, image.host_id);
    }
    commands.shrink_to_fit();
    Some(commands)
}
fn transmit(id: u32, format: u32, width: u32, height: u32, data: &[u8]) -> Option<Vec<u8>> {
    let count = data.len().div_ceil(3072);
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(data.len().div_ceil(3) * 4 + count * 32 + 512)
        .ok()?;
    for (i, chunk) in data.chunks(3072).enumerate() {
        let more = u8::from(i + 1 < count);
        if i == 0 {
            commands.extend_from_slice(
                format!("\x1b_Ga=t,t=d,i={id},f={format},s={width},v={height},q=2,m={more};")
                    .as_bytes(),
            );
        } else {
            commands.extend_from_slice(format!("\x1b_Gq=2,m={more};").as_bytes());
        }
        commands.extend_from_slice(&encoded(chunk));
        commands.extend_from_slice(b"\x1b\\");
    }
    Some(commands)
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut n = 0;
        while n < 8 {
            c = if c & 1 != 0 {
                0xedb88320 ^ (c >> 1)
            } else {
                c >> 1
            };
            n += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}
fn crc(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table();
    let mut c = !0u32;
    for &b in bytes {
        c = TABLE[((c as u8) ^ b) as usize] ^ (c >> 8);
    }
    !c
}
fn be(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}
fn sanitize_png(input: &[u8]) -> Option<(Vec<u8>, u32, u32, usize)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if input.len() > MAX_INPUT || !input.starts_with(SIGNATURE) {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(input.len()).ok()?;
    out.extend_from_slice(SIGNATURE);
    let mut compressed = Vec::new();
    compressed.try_reserve_exact(input.len()).ok()?;
    let mut at = 8;
    let mut dimensions = None;
    let (mut palette, mut transparency, mut seen_data, mut closed_data) =
        (None, false, false, false);
    while at < input.len() {
        let len = be(input.get(at..at + 4)?)? as usize;
        let end = at.checked_add(12)?.checked_add(len)?;
        let chunk = input.get(at..end)?;
        let kind = &chunk[4..8];
        let data = &chunk[8..8 + len];
        if !kind.iter().all(u8::is_ascii_alphabetic)
            || kind[2].is_ascii_lowercase()
            || crc(&chunk[4..8 + len]) != be(&chunk[8 + len..])?
        {
            return None;
        }
        if dimensions.is_none() && kind != b"IHDR" {
            return None;
        }
        if seen_data && kind != b"IDAT" {
            closed_data = true;
        }
        let keep = match kind {
            b"IHDR" => {
                if dimensions.is_some() || len != 13 {
                    return None;
                }
                let w = be(&data[..4])?;
                let h = be(&data[4..8])?;
                let depth = data[8];
                let color = data[9];
                let valid = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(depth, 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    _ => false,
                };
                if !valid || data[10] != 0 || data[11] != 0 || data[12] > 1 {
                    return None;
                }
                let bytes = footprint(w, h, if depth == 16 { 8 } else { 4 })?;
                dimensions = Some((w, h, depth, color, data[12], bytes));
                true
            }
            b"PLTE" => {
                let (_, _, depth, color, _, _) = dimensions?;
                if palette.is_some()
                    || seen_data
                    || matches!(color, 0 | 4)
                    || len == 0
                    || len > 768
                    || len % 3 != 0
                    || (color == 3 && len / 3 > (1usize << depth))
                {
                    return None;
                }
                palette = Some(len / 3);
                true
            }
            b"tRNS" => {
                let (_, _, _, color, _, _) = dimensions?;
                if transparency
                    || seen_data
                    || !match color {
                        0 => len == 2,
                        2 => len == 6,
                        3 => len > 0 && len <= palette?,
                        _ => false,
                    }
                {
                    return None;
                }
                transparency = true;
                true
            }
            b"IDAT" => {
                let (_, _, _, color, _, _) = dimensions?;
                if closed_data || (color == 3 && palette.is_none()) {
                    return None;
                }
                if compressed.len().checked_add(len)? > MAX_INPUT {
                    return None;
                }
                compressed.extend_from_slice(data);
                seen_data = true;
                true
            }
            b"IEND" => {
                if len != 0 || !seen_data || end != input.len() {
                    return None;
                }
                let (w, h, depth, color, interlace, bytes) = dimensions?;
                validate_scanlines(&compressed, w, h, depth, color, interlace)?;
                out.extend_from_slice(chunk);
                out.shrink_to_fit();
                return Some((out, w, h, bytes));
            }
            _ if kind[0].is_ascii_uppercase() => return None,
            _ => false,
        };
        if keep {
            out.extend_from_slice(chunk);
        }
        at = end;
    }
    None
}
fn validate_scanlines(
    compressed: &[u8],
    w: u32,
    h: u32,
    depth: u8,
    color: u8,
    interlace: u8,
) -> Option<()> {
    let channels = match color {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let mut passes = Vec::new();
    let steps: &[(u32, u32, u32, u32)] = if interlace == 0 {
        &[(0, 0, 1, 1)]
    } else {
        &[
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
    };
    let mut expected = 0usize;
    for &(x, y, dx, dy) in steps {
        if w <= x || h <= y {
            continue;
        }
        let cols = (w - x).div_ceil(dx) as usize;
        let rows = (h - y).div_ceil(dy) as usize;
        let row = (cols * channels * depth as usize).div_ceil(8);
        expected = expected.checked_add((row + 1).checked_mul(rows)?)?;
        passes.push((row, rows));
    }
    if expected > MAX_INPUT + MAX_AXIS as usize * 2 {
        return None;
    }
    let mut decoded = Vec::new();
    decoded.try_reserve_exact(expected).ok()?;
    decoded.resize(expected, 0);
    let mut state =
        miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Zlib);
    let result = miniz_oxide::inflate::stream::inflate(
        &mut state,
        compressed,
        &mut decoded,
        miniz_oxide::MZFlush::Finish,
    );
    if result.status != Ok(miniz_oxide::MZStatus::StreamEnd)
        || result.bytes_written != expected
        || result.bytes_consumed != compressed.len()
    {
        return None;
    }
    let mut at = 0;
    for (row, rows) in passes {
        for _ in 0..rows {
            if decoded[at] > 4 {
                return None;
            }
            at += row + 1;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apc(header: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = format!("\x1b_G{header};").into_bytes();
        out.extend_from_slice(payload);
        out.extend_from_slice(b"\x1b\\");
        out
    }
    fn rgb(l: &mut Legacy, id: u32) {
        l.accept(&apc(
            &format!("a=T,U=1,i={id},p=1,f=24,s=1,v=1,c=1,r=1"),
            b"AQID",
        ));
    }
    fn mark(id: u32, row: usize, col: usize) -> Cell {
        let mut cell = Cell {
            ch: PLACEHOLDER,
            ..Cell::default()
        };
        cell.style.fg = id_color(id);
        cell.combining[0] = DIACRITICS[row];
        cell.combining[1] = DIACRITICS[col];
        cell.combining[2] = DIACRITICS[(id >> 24) as usize];
        cell
    }
    fn source(image: &Image) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = &image.commands[..image.source_end];
        while !rest.is_empty() {
            let end = rest.windows(2).position(|w| w == b"\x1b\\").unwrap() + 2;
            let (h, p) = parse(&rest[..end]).unwrap();
            assert_eq!(h.number(b'q', 0), Some(2));
            assert!(h.allowed(b"atifsvqm"));
            if h.0.contains_key(&b'i') {
                assert_eq!(h.number(b'i', 0), Some(image.host_id));
                assert_eq!(h.letter(b't', 0), Some(b'd'));
            }
            out.extend(decode(p, h.more().unwrap()).unwrap());
            rest = &rest[end..];
        }
        out
    }
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let checksum = crc(&out[4..]);
        out.extend_from_slice(&checksum.to_be_bytes());
        out
    }
    fn png(
        w: u32,
        h: u32,
        depth: u8,
        color: u8,
        interlace: u8,
        raw: &[u8],
        extra: &[u8],
    ) -> Vec<u8> {
        let mut header = w.to_be_bytes().to_vec();
        header.extend_from_slice(&h.to_be_bytes());
        header.extend_from_slice(&[depth, color, 0, 0, interlace]);
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(raw, 1);
        png_compressed(&header, &compressed, extra)
    }
    fn png_compressed(header: &[u8], compressed: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend(chunk(b"IHDR", header));
        p.extend_from_slice(extra);
        p.extend(chunk(b"IDAT", compressed));
        p.extend(chunk(b"IEND", &[]));
        p
    }

    #[test]
    fn forbidden_media_compression_and_controls_never_reach_host() {
        for extra in [
            "t=f", "t=t", "t=s", "o=z", "P=1", "Q=1", "H=1", "V=1", "S=9", "O=2", "a=f", "a=a",
            "a=c", "f=99", "i=0", "I=3", "z=1", "U=2",
        ] {
            let mut l = Legacy::default();
            let mut h = Host::default();
            l.accept(&apc(&format!("a=T,U=1,i=7,f=24,s=1,v=1,{extra}"), b"AQID"));
            assert!(l.images.is_empty(), "accepted {extra}");
            assert_eq!(l.cell(mark(7, 0, 0), &mut h).ch, ' ');
            assert!(h.pending.is_empty());
        }
    }

    #[test]
    fn all_aligned_raw_chunk_splits_are_atomic_and_canonical() {
        let payload = encoded(&(0..255u8).cycle().take(3600).collect::<Vec<_>>());
        for split in (704..=4096).step_by(4) {
            let mut l = Legacy::default();
            l.accept(&apc(
                "a=T,U=1,i=7,p=1,f=24,s=1200,v=1,c=120,r=1,m=1",
                &payload[..split],
            ));
            assert!(l.images.is_empty());
            assert!(l.pending.is_some());
            l.accept(&apc("m=0,q=2", &payload[split..]));
            let image = &l.images[&7];
            assert_eq!(
                source(image),
                (0..255u8).cycle().take(3600).collect::<Vec<_>>()
            );
            assert_eq!(image.footprint, 4800);
            assert_eq!(image.placements.len(), 1);
            assert_ne!(image.host_id, 7);
            assert!(l.pending.is_none());
        }
    }

    #[test]
    fn malformed_and_aborted_transfers_cannot_replace_or_splice_sources() {
        let mut l = Legacy::default();
        rgb(&mut l, 7);
        let old = l.images[&7].host_id;
        for payload in [
            b"AQI=".as_slice(),
            b"AQIDBA==",
            b"AR==",
            b"AQI=AAAA",
            b"@@@@",
            b"AQID\x1b[2J",
        ] {
            l.accept(&apc("a=T,U=1,i=7,f=24,s=1,v=1", payload));
            assert_eq!(l.images[&7].host_id, old);
        }
        l.accept(&apc("a=T,U=1,i=8,f=24,s=1,v=2,m=1", b"AQID"));
        l.accept(&apc("m=0,t=f", b"BAUG"));
        assert!(!l.images.contains_key(&8));
        l.accept(&apc("a=T,U=1,i=8,f=24,s=1,v=2,m=1", b"AQID"));
        l.abort_transfer();
        l.accept(&apc("m=0", b"BAUG"));
        assert!(!l.images.contains_key(&8));
        l.accept(&apc("a=T,U=1,i=8,f=24,s=1,v=2,m=1", b"AQID"));
        l.delete_command(&apc("a=t,i=9,f=24,s=1,v=1", b"AQID"));
        l.accept(&apc("m=0", b"BAUG"));
        assert!(!l.images.contains_key(&8));
        rgb(&mut l, 8);
        assert!(l.images.contains_key(&8));
    }

    #[test]
    fn rgba_and_transmit_then_virtual_place_work() {
        let mut l = Legacy::default();
        l.accept(&apc("a=t,U=1,i=7,f=32,s=1,v=1", b"AQIDBA=="));
        assert_eq!(source(&l.images[&7]), vec![1, 2, 3, 4]);
        assert!(l.images[&7].placements.is_empty());
        l.accept(&apc("a=p,U=1,i=7,p=4,c=2,r=3", b""));
        assert_eq!(l.images[&7].placements.len(), 1);
        let mut host = Host::default();
        let mut cell = mark(7, 0, 0);
        cell.style.underline_color = CColor::Rgb(0, 0, 4);
        assert_eq!(l.cell(cell, &mut host).ch, PLACEHOLDER);
        assert!(!host.pending.is_empty());
    }

    #[test]
    fn changed_placement_reuploads_immediately_and_replacement_gets_new_id() {
        let mut l = Legacy::default();
        let mut h = Host::default();
        rgb(&mut l, 7);
        let old_id = l.images[&7].host_id;
        let old_revision = l.images[&7].revision;
        h.begin();
        l.cell(mark(7, 0, 0), &mut h);
        assert!(!h.pending.is_empty());
        h.finish();
        h.begin();
        l.cell(mark(7, 0, 0), &mut h);
        assert!(h.pending.is_empty());
        h.finish();
        l.accept(&apc("a=p,U=1,i=7,p=1,c=2,r=3", b""));
        assert!(l.images[&7].revision > old_revision);
        assert_eq!(l.images[&7].host_id, old_id);
        h.begin();
        l.cell(mark(7, 0, 0), &mut h);
        assert!(!h.pending.is_empty());
        h.finish();
        rgb(&mut l, 7);
        assert_ne!(l.images[&7].host_id, old_id);
    }

    #[test]
    fn deletes_are_scoped_preserve_sources_and_respect_virtual_templates() {
        let mut a = Legacy::default();
        let mut b = Legacy::default();
        rgb(&mut a, 7);
        rgb(&mut b, 7);
        assert_ne!(a.images[&7].host_id, b.images[&7].host_id);
        let original = source(&a.images[&7]);
        for mode in ["a", "A"] {
            a.delete_command(&apc(&format!("a=d,d={mode}"), b""));
            assert_eq!(a.images[&7].placements.len(), 1);
        }
        a.accept(&apc("a=p,U=1,i=7,p=2,c=1,r=1", b""));
        a.delete_command(&apc("a=d,d=I,i=7,p=1", b""));
        assert_eq!(a.images[&7].placements.len(), 1);
        assert_eq!(a.images[&7].placements[0].id, 2);
        a.delete_command(&apc("a=d,d=i,i=7", b""));
        assert!(a.images[&7].placements.is_empty());
        assert_eq!(source(&a.images[&7]), original);
        a.accept(&apc("a=p,U=1,i=7,p=3,c=1,r=1", b""));
        assert_eq!(a.images[&7].placements.len(), 1);
        a.delete_command(&apc("a=d,d=R,x=7,y=7", b""));
        assert!(a.images.is_empty());
        assert_eq!(a.bytes, 0);
        assert!(b.images.contains_key(&7));
    }

    #[test]
    fn every_delete_aborts_pending_including_physical_and_unsupported_deletes() {
        for mode in ["a", "A", "i", "I", "r", "R", "z", "Z", "?"] {
            let mut l = Legacy::default();
            l.accept(&apc("a=T,U=1,i=7,f=24,s=1,v=2,m=1", b"AQID"));
            l.delete_command(&apc(&format!("a=d,d={mode},i=7,x=7,y=7"), b""));
            assert!(l.pending.is_none());
            rgb(&mut l, 8);
            assert!(l.images.contains_key(&8), "pending survived d={mode}");
        }
    }

    #[test]
    fn sparse_marks_inherit_row_column_high_byte_and_survive_clipping() {
        let id = (2 << 24) | 7;
        let mut l = Legacy::default();
        let mut h = Host::default();
        rgb(&mut l, id);
        l.begin_row();
        l.observe(mark(id, 3, 0));
        assert!(h.pending.is_empty());
        let mut sparse = mark(id, 3, 1);
        sparse.combining = NO_COMBINING;
        let mapped = l.cell(sparse, &mut h);
        assert_eq!(mapped.ch, PLACEHOLDER);
        assert_eq!(mapped.combining[0], DIACRITICS[3]);
        assert_eq!(mapped.combining[1], DIACRITICS[1]);
        let mut glyph = String::new();
        mapped.push_codepoints(&mut glyph);
        assert_eq!(glyph.chars().count(), 4);
        sparse.combining[0] = DIACRITICS[3];
        let mapped = l.cell(sparse, &mut h);
        assert_eq!(mapped.combining[1], DIACRITICS[2]);
        l.begin_row();
        assert_eq!(l.cell(sparse, &mut h).ch, ' '); // High byte cannot inherit across rows.
        rgb(&mut l, 7);
        l.begin_row();
        assert_eq!(l.cell(sparse, &mut h).combining[1], DIACRITICS[0]);
        l.observe(Cell::default());
        assert_eq!(l.cell(sparse, &mut h).combining[1], DIACRITICS[0]);
    }

    #[test]
    fn image_placement_and_allocated_source_budgets_are_enforced() {
        let mut l = Legacy::default();
        for id in 1..=MAX_IMAGES as u32 {
            rgb(&mut l, id);
        }
        rgb(&mut l, 999);
        assert_eq!(l.images.len(), MAX_IMAGES);
        rgb(&mut l, 1);
        assert_eq!(l.images.len(), MAX_IMAGES);
        let mut l = Legacy::default();
        rgb(&mut l, 1);
        for p in 2..=MAX_PLACEMENTS as u32 {
            l.accept(&apc(&format!("a=p,U=1,i=1,p={p},c=1,r=1"), b""));
        }
        l.accept(&apc("a=p,U=1,i=1,p=999,c=1,r=1", b""));
        assert_eq!(l.count(), MAX_PLACEMENTS);
        l.accept(&apc("a=p,U=1,i=1,p=1,c=2,r=1", b""));
        assert_eq!(l.images[&1].placements[0].cols, 2);
        let image = l.images.get_mut(&1).unwrap();
        image
            .commands
            .reserve_exact(MAX_RETAINED - image.commands.len());
        l.bytes = image.commands.capacity();
        assert!(l.bytes >= MAX_RETAINED);
        rgb(&mut l, 2);
        assert!(!l.images.contains_key(&2));
        assert!(footprint(16384, 16384, 4).is_none());
        assert!(footprint(0, 1, 4).is_none());
        assert!(footprint(u32::MAX, 1, 4).is_none());
    }

    #[test]
    fn mixed_mode_child_ids_switch_only_after_successful_source_commit() {
        fn route(g: &mut crate::graphics::Graphics, l: &mut Legacy, header: &str, payload: &[u8]) {
            let packet = apc(header, payload);
            let outcome = g.command(&packet, (0, 0), (1, 1));
            l.delete_command(&packet);
            if let Some(id) = outcome.committed_image {
                l.forget_image(id);
            }
            if outcome.legacy {
                l.accept(&packet);
                if let Some(id) = l.take_committed_image() {
                    g.forget_image(id);
                }
            }
        }
        let mut g = crate::graphics::Graphics::default();
        let mut l = Legacy::default();
        route(&mut g, &mut l, "a=T,i=7,f=24,s=1,v=1,C=1", b"AQID");
        assert!(g.source(7).is_some());
        route(&mut g, &mut l, "a=T,U=1,i=7,f=24,s=1,v=1,t=f,q=2", b"AQID");
        assert!(g.source(7).is_some());
        assert!(l.images.is_empty());
        route(&mut g, &mut l, "a=T,U=1,i=7,f=24,s=1,v=2,m=1,q=2", b"AQID");
        assert!(g.source(7).is_some());
        assert!(l.images.is_empty());
        route(&mut g, &mut l, "m=0,q=0", b"BAUG");
        assert!(g.source(7).is_none());
        assert!(g.placements().is_empty());
        assert_eq!(source(&l.images[&7]), vec![1, 2, 3, 4, 5, 6]);
        let host_id = l.images[&7].host_id;
        for (header, data) in [
            ("a=q,i=7,f=24,s=1,v=1", b"AQID".as_slice()),
            ("a=p,i=7,C=1", b""),
            ("a=T,i=7,f=24,s=1,v=1,C=1", b"AQI="),
        ] {
            route(&mut g, &mut l, header, data);
            assert_eq!(l.images[&7].host_id, host_id);
        }
        route(&mut g, &mut l, "a=T,i=7,f=32,s=1,v=1,C=1", b"AQIDBA==");
        assert!(g.source(7).is_some());
        assert!(l.images.is_empty());
        assert_eq!(l.bytes, 0);
    }

    #[test]
    fn legacy_commit_markers_exclude_updates_and_rejections_and_forget_is_scoped() {
        let mut l = Legacy::default();
        rgb(&mut l, 7);
        assert_eq!(l.take_committed_image(), Some(7));
        assert_eq!(l.take_committed_image(), None);
        l.accept(&apc("a=p,U=1,i=7,p=2,c=1,r=1", b""));
        assert_eq!(l.take_committed_image(), None);
        l.accept(&apc("a=T,U=1,i=7,f=24,s=1,v=1", b"AQI="));
        assert_eq!(l.take_committed_image(), None);
        rgb(&mut l, 8);
        assert_eq!(l.take_committed_image(), Some(8));
        let kept = l.images[&8].commands.capacity();
        l.forget_image(7);
        assert!(!l.images.contains_key(&7));
        assert!(l.images.contains_key(&8));
        assert_eq!(l.bytes, kept);
        l.forget_image(7);
        assert_eq!(l.bytes, kept);
        l.accept(&apc("a=T,U=1,i=9,f=24,s=1,v=2,m=1,q=2", b"AQID"));
        l.delete_command(&apc("a=p,U=1,i=7,q=1", b""));
        assert!(l.pending.is_none());
    }

    #[test]
    fn png_is_validated_sanitized_and_transmitted_canonically() {
        assert_eq!(crc(b"123456789"), 0xcbf43926);
        let raw = [0, 255, 0, 0, 255];
        let clean = png(1, 1, 8, 6, 0, &raw, &[]);
        let mut extra = chunk(b"tEXt", b"secret\0not forwarded");
        extra.extend(chunk(b"iCCP", b"ignored unsafe compressed profile"));
        let with_metadata = png(1, 1, 8, 6, 0, &raw, &extra);
        let (sanitized, w, h, bytes) = sanitize_png(&with_metadata).unwrap();
        assert_eq!(sanitized, clean);
        assert_eq!((w, h, bytes), (1, 1, 4));
        let mut l = Legacy::default();
        l.accept(&apc("a=T,U=1,i=7,f=100,c=1,r=1", &encoded(&with_metadata)));
        assert_eq!(source(&l.images[&7]), clean);
        assert_eq!(l.images[&7].footprint, 4);
    }

    #[test]
    fn png_huge_headers_crc_critical_chunks_bad_order_and_truncation_rejected() {
        let valid = png(1, 1, 8, 6, 0, &[0, 1, 2, 3, 255], &[]);
        for n in 0..valid.len() {
            assert!(
                sanitize_png(&valid[..n]).is_none(),
                "accepted truncated prefix {n}"
            );
        }
        let mut bad = valid.clone();
        bad[29] ^= 1;
        assert!(sanitize_png(&bad).is_none());
        assert!(sanitize_png(&png(16384, 16384, 8, 6, 0, &[0], &[])).is_none());
        assert!(sanitize_png(&png(
            1,
            1,
            8,
            6,
            0,
            &[0, 1, 2, 3, 255],
            &chunk(b"ABCD", b"")
        ))
        .is_none());
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(sanitize_png(&trailing).is_none());
        let mut wrong_order = b"\x89PNG\r\n\x1a\n".to_vec();
        wrong_order.extend(chunk(b"IDAT", &[]));
        assert!(sanitize_png(&wrong_order).is_none());
        assert!(sanitize_png(&png(1, 1, 7, 6, 0, &[0], &[])).is_none());
        assert!(sanitize_png(&png(1, 1, 8, 6, 2, &[0], &[])).is_none());
    }

    #[test]
    fn png_actual_inflation_filter_bytes_checksum_and_trailing_streams_are_bounded() {
        assert!(sanitize_png(&png(1, 1, 8, 6, 0, &vec![0; 1024 * 1024], &[])).is_none());
        assert!(sanitize_png(&png(1, 1, 8, 6, 0, &[0, 1], &[])).is_none());
        assert!(sanitize_png(&png(1, 1, 8, 6, 0, &[5, 1, 2, 3, 255], &[])).is_none());
        let header = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];
        let mut compressed = miniz_oxide::deflate::compress_to_vec_zlib(&[0, 1, 2, 3, 255], 1);
        let n = compressed.len();
        compressed[n - 1] ^= 1;
        assert!(sanitize_png(&png_compressed(&header, &compressed, &[])).is_none());
        compressed[n - 1] ^= 1;
        compressed.extend(miniz_oxide::deflate::compress_to_vec_zlib(
            &vec![0; 65536],
            1,
        ));
        assert!(sanitize_png(&png_compressed(&header, &compressed, &[])).is_none());
    }

    #[test]
    fn png_palette_transparency_16bit_and_adam7_are_supported() {
        let mut palette = chunk(b"PLTE", &[255, 0, 0, 0, 255, 0]);
        palette.extend(chunk(b"tRNS", &[255, 128]));
        assert!(sanitize_png(&png(3, 1, 1, 3, 0, &[0, 0b01000000], &palette)).is_some());
        assert!(sanitize_png(&png(3, 1, 1, 3, 0, &[0, 0], &[])).is_none());
        assert_eq!(
            sanitize_png(&png(1, 1, 16, 0, 0, &[0, 255, 255], &[]))
                .unwrap()
                .3,
            8
        );
        // Eight-by-eight RGBA Adam7 has 64*4 pixel bytes and 15 filter bytes.
        assert!(sanitize_png(&png(8, 8, 8, 6, 1, &vec![0; 271], &[])).is_some());
        assert!(sanitize_png(&png(8, 8, 8, 6, 1, &vec![0; 270], &[])).is_none());
    }
}
