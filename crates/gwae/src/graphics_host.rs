//! Owned host image cache and scene overlay. Never writes image cells into a
//! child's terminal grid. Host ids are shared across traditional and legacy
//! virtual placements, and every host command is quiet to avoid stdin leaks.
use crate::graphics::{Graphics, Placement, Source};
use crate::graphics_diacritics::DIACRITICS;
use gwae_term::{CColor, Cell, Style, NO_COMBINING};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const PLACEHOLDER: char = '\u{10eeee}';
const REFRESH: Duration = Duration::from_secs(30);
/// How long after a reveal the self-heal re-upload fires when the texture
/// went refresh-stale while hidden. The reveal itself must stay free (that
/// is the point of retaining hidden textures), but the 30s heal exists
/// because a host can silently drop images; deferring the heal a few
/// seconds puts the multi-MiB re-upload in an idle frame while the user is
/// reading instead of inside the navigation paint, and bounds how long a
/// genuinely dropped image stays blank after a reveal.
const REVEAL_HEAL: Duration = Duration::from_secs(5);
const MAX_RASTER: usize = 32 * 1024 * 1024;
const MAX_HOST_BYTES: usize = 128 * 1024 * 1024;
static NEXT_ID: AtomicU32 = AtomicU32::new(0x4700_0001);
pub(crate) fn allocate() -> u32 {
    // A single process-wide namespace is essential: two panes routinely use i=1.
    NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("host image id namespace exhausted")
}
fn delete(buf: &mut Vec<u8>, id: u32) {
    buf.extend_from_slice(format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").as_bytes());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub row: u16,
    pub col: u16,
    pub rows: u16,
    pub cols: u16,
    pub id: u32,
    pub z: i32,
}
impl Tile {
    pub fn cell(&self, row: u16, col: u16, underneath: Cell) -> Option<Cell> {
        let y = row.checked_sub(self.row)?;
        let x = col.checked_sub(self.col)?;
        if y >= self.rows || x >= self.cols || (self.z < 0 && underneath.ch != ' ') {
            return None;
        }
        let mut combining = NO_COMBINING;
        combining[0] = DIACRITICS[y as usize];
        combining[1] = DIACRITICS[x as usize];
        combining[2] = DIACRITICS[(self.id >> 24) as usize];
        Some(Cell {
            ch: PLACEHOLDER,
            combining,
            width: 1,
            style: Style {
                fg: id_color(self.id),
                bg: underneath.style.bg,
                underline_color: CColor::Rgb(0, 0, 1),
                ..Style::default()
            },
        })
    }
}
pub(crate) fn id_color(id: u32) -> CColor {
    CColor::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
}
#[cfg(test)]
fn child_id(cell: Cell) -> u32 {
    let low = match cell.style.fg {
        CColor::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        CColor::Idx(n) => n as u32,
        CColor::Default => 0,
    };
    let high = DIACRITICS
        .iter()
        .position(|&c| c == cell.combining[2])
        .unwrap_or(0) as u32;
    low | (high << 24)
}

struct Texture {
    id: u32,
    epoch: u64,
    uploaded: Instant,
    bytes: usize,
}
struct VirtualTexture {
    epoch: u64,
    uploaded: Instant,
    revision: u64,
    bytes: usize,
    /// Owning pane, for `finish()`'s painted-pane staleness rule. One legacy
    /// transfer belongs to exactly one pane (host ids are never shared).
    pane: u64,
}
/// Key for `Host::prepare_cache`: pane id, image-activity token, view rect
/// (col, row, w, h), and cell pixel size (w, h).
type PrepareKey = (u64, u64, u16, u16, u16, u16, u16, u16);
#[derive(Default)]
pub struct Host {
    epoch: u64,
    textures: HashMap<Vec<u64>, Texture>,
    legacy: HashMap<u32, VirtualTexture>,
    bytes: usize,
    pub pending: Vec<u8>,
    /// Phase 1 image isolation: per-pane prepared frame cache. Keyed by pane
    /// id plus the pane's image-activity token, view rect, and cell size, so
    /// a still image pane reuses its tiles without rewalking sources,
    /// resorting placements, or rebuilding tile rects every frame. Entries
    /// are invalidated by token change, geometry change, or `clear()`, and
    /// culled by `finish()` when unused for a frame or pointing at an
    /// evicted texture. Each entry carries the epoch it last served, so a
    /// hidden pane's entry (rebuildable in microseconds) never outlives its
    /// frame while the expensive host textures are retained independently.
    prepare_cache: HashMap<PrepareKey, (u64, Vec<Tile>)>,
    /// Panes prepared this frame. `finish()` deletes stale textures only for
    /// panes in this set: a painted pane that stopped using a texture proves
    /// it dead (child delete, geometry change), while an unpainted pane's
    /// textures are the hidden-strip cache and must survive the frame.
    painted: std::collections::HashSet<u64>,
}
impl Host {
    pub fn refresh_due(&self) -> bool {
        // Only textures painted in the previous frame are refresh
        // candidates. A retained hidden texture cannot be re-uploaded until
        // its pane is painted again (`prepare`/`show_legacy` own the upload),
        // so counting it here would mark every frame dirty forever: a
        // repaint busy-loop that never clears the condition.
        self.textures
            .values()
            .any(|t| t.epoch == self.epoch && t.uploaded.elapsed() >= REFRESH)
            || self
                .legacy
                .values()
                .any(|t| t.epoch == self.epoch && t.uploaded.elapsed() >= REFRESH)
    }
    pub fn begin(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.pending.clear();
        self.painted.clear();
    }
    fn budget(&mut self, required: usize) -> bool {
        if required > MAX_HOST_BYTES {
            return false;
        }
        if self.bytes + required <= MAX_HOST_BYTES {
            return true;
        }
        // Retained textures are a cache of hidden panes' images; evict the
        // least recently painted first (its pane is the least likely to be
        // revealed next). Entries touched this epoch are never victims: they
        // back the frame being prepared right now, and a replaced or hidden
        // texture must not block its own successor, otherwise no later dirty
        // frame may exist to retry it.
        enum Victim {
            Native(Vec<u64>),
            Legacy(u32),
        }
        let mut victims: Vec<(u64, Victim)> = self
            .textures
            .iter()
            .filter(|(_, t)| t.epoch != self.epoch)
            .map(|(k, t)| (t.epoch, Victim::Native(k.clone())))
            .chain(
                self.legacy
                    .iter()
                    .filter(|(_, t)| t.epoch != self.epoch)
                    .map(|(&id, t)| (t.epoch, Victim::Legacy(id))),
            )
            .collect();
        victims.sort_by_key(|&(epoch, _)| epoch);
        for (_, victim) in victims {
            match victim {
                Victim::Native(key) => {
                    let t = self.textures.remove(&key).unwrap();
                    delete(&mut self.pending, t.id);
                    self.bytes -= t.bytes;
                }
                Victim::Legacy(id) => {
                    let t = self.legacy.remove(&id).unwrap();
                    delete(&mut self.pending, id);
                    self.bytes -= t.bytes;
                }
            }
            if self.bytes + required <= MAX_HOST_BYTES {
                return true;
            }
        }
        self.bytes + required <= MAX_HOST_BYTES
    }
    /// Grid-space view rectangle (column, row, width, height). Only visible
    /// pieces are rasterized, in <=256-cell tiles with explicit row/column ids.
    ///
    /// Phase 1 cache: when the caller's `image_activity` token matches the
    /// last prepared frame for this pane/view/cell geometry, the cached tile
    /// list is returned after refreshing texture epochs, skipping the source
    /// walk, placement sort, and tile-rect rebuild. Any token change (image
    /// commit, placement update, delete, clear) forces a full prepare. `None`
    /// means the pane never carried image traffic: empty tiles, no walk.
    pub fn prepare_cached(
        &mut self,
        pane: u64,
        image_activity: Option<u64>,
        graphics: &Graphics,
        view: (u16, u16, u16, u16),
        cell: (u16, u16),
    ) -> Vec<Tile> {
        let Some(token) = image_activity else {
            return Vec::new();
        };
        let key = (pane, token, view.0, view.1, view.2, view.3, cell.0, cell.1);
        // The cached path skips `prepare`, which owns the REFRESH re-upload.
        // A still pane served purely from this cache would leave `uploaded`
        // stale forever while `refresh_due()` keeps marking frames dirty: a
        // repaint loop that can never heal the host. When any backing texture
        // is due, drop the entry and take the full path so it re-uploads.
        let refresh = self.prepare_cache.get(&key).is_some_and(|(_, tiles)| {
            tiles.iter().any(|tile| {
                self.textures
                    .values()
                    .any(|t| t.id == tile.id && t.uploaded.elapsed() >= REFRESH)
            })
        });
        if refresh {
            self.prepare_cache.remove(&key);
        }
        if let Some((used, cached)) = self.prepare_cache.get_mut(&key) {
            *used = self.epoch;
            self.painted.insert(pane);
            let tiles = cached.clone();
            // Cached path must still mark textures live for this epoch, or
            // budget eviction could reclaim textures backing a still-visible
            // frame.
            for tile in &tiles {
                for texture in self.textures.values_mut() {
                    if texture.id == tile.id {
                        texture.epoch = self.epoch;
                        break;
                    }
                }
            }
            return tiles;
        }
        let tiles = self.prepare(pane, graphics, view, cell);
        self.prepare_cache.insert(key, (self.epoch, tiles.clone()));
        tiles
    }
    pub fn prepare(
        &mut self,
        pane: u64,
        graphics: &Graphics,
        view: (u16, u16, u16, u16),
        cell: (u16, u16),
    ) -> Vec<Tile> {
        let mut result = Vec::new();
        self.painted.insert(pane);
        if cell.0 == 0 || cell.1 == 0 {
            return result;
        }
        let per_cell = cell.0 as usize * cell.1 as usize * 4;
        let tile_cols = (MAX_RASTER / per_cell).min(256);
        if tile_cols == 0 {
            return result;
        }
        let tile_rows = (MAX_RASTER / per_cell / tile_cols).clamp(1, 256);
        let mut placements: Vec<_> = graphics.placements().iter().collect();
        placements.sort_by_key(|p| (p.z_index, p.image_id, p.placement_id));
        for p in placements {
            let Some(source) = graphics.source(p.image_id) else {
                continue;
            };
            let wide = (p.offset_x as u64 + p.pixel_width as u64).div_ceil(cell.0 as u64);
            let high = (p.offset_y as u64 + p.pixel_height as u64).div_ceil(cell.1 as u64);
            let left = (p.col as u32).max(view.0 as u32);
            let top = (p.row as u32).max(view.1 as u32);
            let right = (p.col as u64 + wide).min(view.0 as u64 + view.2 as u64) as u32;
            let bottom = (p.row as u64 + high).min(view.1 as u64 + view.3 as u64) as u32;
            if left >= right || top >= bottom {
                continue;
            }
            for y in (top..bottom).step_by(tile_rows) {
                for x in (left..right).step_by(tile_cols) {
                    let cols = (right - x).min(tile_cols as u32) as u16;
                    let rows = (bottom - y).min(tile_rows as u32) as u16;
                    let tw = cols as u32 * cell.0 as u32;
                    let th = rows as u32 * cell.1 as u32;
                    let nbytes = tw as u64 * th as u64 * 4;
                    if nbytes > MAX_RASTER as u64 {
                        continue;
                    }
                    let dx = (x - p.col as u32) * cell.0 as u32;
                    let dy = (y - p.row as u32) * cell.1 as u32;
                    let key = vec![
                        pane,
                        source.revision,
                        p.image_id as u64,
                        p.placement_id as u64,
                        p.row as u64,
                        p.col as u64,
                        p.x as u64,
                        p.y as u64,
                        p.width as u64,
                        p.height as u64,
                        p.pixel_width as u64,
                        p.pixel_height as u64,
                        p.offset_x as u64,
                        p.offset_y as u64,
                        dx as u64,
                        dy as u64,
                        tw as u64,
                        th as u64,
                        cols as u64,
                        rows as u64,
                    ];
                    if !self.textures.contains_key(&key) {
                        if !self.budget(nbytes as usize) {
                            continue;
                        }
                        self.bytes += nbytes as usize;
                        self.textures.insert(
                            key.clone(),
                            Texture {
                                id: allocate(),
                                epoch: self.epoch,
                                uploaded: Instant::now() - REFRESH,
                                bytes: nbytes as usize,
                            },
                        );
                    }
                    let texture = self.textures.get_mut(&key).unwrap();
                    if texture.uploaded.elapsed() >= REFRESH {
                        // Freshly inserted textures carry the current epoch
                        // (first upload is immediate); one painted last frame
                        // went stale on screen (heal now, the host may have
                        // dropped it). An OLDER epoch means this is a reveal
                        // after a long hide: keep the reveal frame free and
                        // reschedule the heal via `refresh_due` instead of
                        // paying a multi-MiB re-upload inside the navigation
                        // paint.
                        if texture.epoch == self.epoch
                            || texture.epoch.wrapping_add(1) == self.epoch
                        {
                            let rgba = raster(source, p, dx, dy, tw, th);
                            upload(&mut self.pending, texture.id, &rgba, tw, th, cols, rows);
                            texture.uploaded = Instant::now();
                        } else {
                            texture.uploaded = Instant::now() - REFRESH + REVEAL_HEAL;
                        }
                    }
                    texture.epoch = self.epoch;
                    result.push(Tile {
                        row: y as u16,
                        col: x as u16,
                        rows,
                        cols,
                        id: texture.id,
                        z: p.z_index,
                    });
                }
            }
        }
        result
    }
    pub(crate) fn show_legacy(
        &mut self,
        pane: u64,
        id: u32,
        revision: u64,
        commands: &[u8],
        footprint: usize,
    ) -> bool {
        if self
            .legacy
            .get(&id)
            .is_some_and(|old| old.bytes != footprint)
        {
            let old = self.legacy.remove(&id).unwrap();
            self.bytes -= old.bytes;
            delete(&mut self.pending, id);
        }
        if !self.legacy.contains_key(&id) {
            if !self.budget(footprint) {
                return false;
            }
            self.bytes += footprint;
            self.legacy.insert(
                id,
                VirtualTexture {
                    epoch: self.epoch,
                    uploaded: Instant::now() - REFRESH,
                    revision,
                    bytes: footprint,
                    pane,
                },
            );
        }
        let entry = self.legacy.get_mut(&id).unwrap();
        // A revision change is new content: upload now, whatever the epoch.
        // A refresh-stale texture only uploads now if it was on screen (this
        // or last epoch, including first insert); one that went stale while
        // hidden defers its heal by REVEAL_HEAL so the reveal frame itself
        // ships zero image bytes (see the native path above).
        let visible = entry.epoch == self.epoch || entry.epoch.wrapping_add(1) == self.epoch;
        if entry.revision != revision || (visible && entry.uploaded.elapsed() >= REFRESH) {
            self.pending.extend_from_slice(commands);
            entry.uploaded = Instant::now();
            entry.revision = revision;
        } else if entry.uploaded.elapsed() >= REFRESH {
            entry.uploaded = Instant::now() - REFRESH + REVEAL_HEAL;
        }
        entry.epoch = self.epoch;
        entry.pane = pane;
        true
    }
    pub fn finish(&mut self) {
        let epoch = self.epoch;
        // Staleness is judged per pane, not per frame. A pane that WAS
        // painted this frame and did not use a texture has proven it dead
        // (child delete, replaced source, geometry change): delete it, or a
        // child's scoped delete would leave the host showing a ghost. A pane
        // that was NOT painted (hidden strip, scrolled off, peeked) keeps its
        // textures: they are the hidden-pane image cache that makes strip
        // cycling cost zero image bytes on reveal. A full-pane page re-upload
        // is megabytes of base64 through the single render thread plus a PNG
        // decode in the host, so evicting on hide made every strip
        // round-trip hitch the whole UI. Retention is bounded by
        // MAX_HOST_BYTES: `budget()` reclaims the least recently painted
        // textures first, and `clear()` drops everything on reset.
        let painted = &self.painted;
        let buf = &mut self.pending;
        let bytes = &mut self.bytes;
        // Native texture keys carry the owning pane as their first element.
        self.textures.retain(|key, t| {
            if t.epoch == epoch || !painted.contains(&key[0]) {
                true
            } else {
                delete(buf, t.id);
                *bytes -= t.bytes;
                false
            }
        });
        self.legacy.retain(|id, t| {
            if t.epoch == epoch || !painted.contains(&t.pane) {
                true
            } else {
                delete(buf, *id);
                *bytes -= t.bytes;
                false
            }
        });
        // Drop cached tile lists that were not served this frame or whose
        // texture ids no longer exist (evicted by budget). Tile lists are
        // rebuilt in microseconds, so unlike textures they are not worth
        // retaining for hidden panes, and a stale list would point the
        // composer at a texture the budget may since have deleted.
        let live: std::collections::HashSet<u32> = self.textures.values().map(|t| t.id).collect();
        self.prepare_cache
            .retain(|_, (used, tiles)| *used == epoch && tiles.iter().all(|t| live.contains(&t.id)));
    }
    pub fn clear(&mut self) -> Vec<u8> {
        let mut buf = Vec::new();
        for t in self.textures.values() {
            delete(&mut buf, t.id);
        }
        for &id in self.legacy.keys() {
            delete(&mut buf, id);
        }
        self.textures.clear();
        self.legacy.clear();
        self.bytes = 0;
        self.pending.clear();
        self.prepare_cache.clear();
        buf
    }
    /// Free retained textures whose pane no longer exists. With hidden-pane
    /// retention a closed pane's textures would otherwise linger until budget
    /// pressure; the caller sweeps this after pane reconciliation. Returns
    /// the delete commands for the caller to write out directly (like
    /// `clear()`): `pending` is cleared by the next `begin()`, so deletes
    /// parked there between frames would be silently dropped.
    #[must_use = "the delete commands must be written to the host"]
    pub fn retain_panes(&mut self, live: &std::collections::HashSet<u64>) -> Vec<u8> {
        let mut buf = Vec::new();
        let bytes = &mut self.bytes;
        self.textures.retain(|key, t| {
            if live.contains(&key[0]) {
                true
            } else {
                delete(&mut buf, t.id);
                *bytes -= t.bytes;
                false
            }
        });
        self.legacy.retain(|id, t| {
            if live.contains(&t.pane) {
                true
            } else {
                delete(&mut buf, *id);
                *bytes -= t.bytes;
                false
            }
        });
        let ids: std::collections::HashSet<u32> = self.textures.values().map(|t| t.id).collect();
        self.prepare_cache
            .retain(|key, (_, tiles)| live.contains(&key.0) && tiles.iter().all(|t| ids.contains(&t.id)));
        buf
    }
}

/// Bilinear crop/scale into an exact, transparent-padded cell rectangle.
/// Host virtual-placement crop semantics differ, so the host only sees the
/// final raster. Source images remain owned locally for eviction/reveal retries.
fn raster(s: &Source, p: &Placement, dx: u32, dy: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0; w as usize * h as usize * 4];
    let channels = (s.format / 8) as usize;
    for y in 0..h {
        let yy = dy as i64 + y as i64 - p.offset_y as i64;
        if yy < 0 || yy >= p.pixel_height as i64 {
            continue;
        }
        let sy = (p.y as f64 + (yy as f64 + 0.5) * p.height as f64 / p.pixel_height as f64 - 0.5)
            .clamp(p.y as f64, (p.y + p.height - 1) as f64);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min((p.y + p.height - 1) as usize);
        let fy = sy - y0 as f64;
        for x in 0..w {
            let xx = dx as i64 + x as i64 - p.offset_x as i64;
            if xx < 0 || xx >= p.pixel_width as i64 {
                continue;
            }
            let sx = (p.x as f64 + (xx as f64 + 0.5) * p.width as f64 / p.pixel_width as f64 - 0.5)
                .clamp(p.x as f64, (p.x + p.width - 1) as f64);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min((p.x + p.width - 1) as usize);
            let fx = sx - x0 as f64;
            let base = (y as usize * w as usize + x as usize) * 4;
            for c in 0..4 {
                if c == 3 && channels == 3 {
                    out[base + c] = 255;
                    continue;
                }
                let at = |xx: usize, yy: usize| {
                    s.pixels[(yy * s.width as usize + xx) * channels + c] as f64
                };
                let upper = at(x0, y0) * (1. - fx) + at(x1, y0) * fx;
                let lower = at(x0, y1) * (1. - fx) + at(x1, y1) * fx;
                out[base + c] = (upper * (1. - fy) + lower * fy).round() as u8;
            }
        }
    }
    out
}
pub fn encode(data: &[u8]) -> Vec<u8> {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        out.extend_from_slice(&[
            ABC[a >> 2],
            ABC[((a & 3) << 4) | (b >> 4)],
            if chunk.len() > 1 {
                ABC[((b & 15) << 2) | (c >> 6)]
            } else {
                b'='
            },
            if chunk.len() > 2 { ABC[c & 63] } else { b'=' },
        ]);
    }
    out
}
fn upload(buf: &mut Vec<u8>, id: u32, rgba: &[u8], w: u32, h: u32, cols: u16, rows: u16) {
    // A full-pane page raster is several megabytes; base64 alone would push
    // ~7.4 MB per redraw through the single render thread and the host's
    // parser, which is what made image-heavy panes (and therefore the whole
    // UI, since one thread paints every pane) feel sluggish. Page rasters are
    // hugely redundant, so the cheapest zlib level shrinks them by two orders
    // of magnitude for a fraction of the time base64 alone already cost.
    // `o=z` is the Kitty protocol's own zlib option; hosts that lack it never
    // get image output in the first place, since we only emit tiles when the
    // host advertised graphics support.
    let deflated = miniz_oxide::deflate::compress_to_vec_zlib(rgba, 1);
    let (encoded, compressed) = if deflated.len() < rgba.len() {
        (encode(&deflated), true)
    } else {
        (encode(rgba), false)
    };
    let o = if compressed { ",o=z" } else { "" };
    let count = encoded.len().div_ceil(4096);
    for (n, chunk) in encoded.chunks(4096).enumerate() {
        let more = u8::from(n + 1 < count);
        if n == 0 {
            buf.extend_from_slice(
                format!(
                    "\x1b_Ga=T,U=1,p=1,i={id},f=32,s={w},v={h},c={cols},r={rows}{o},q=2,m={more};"
                )
                .as_bytes(),
            );
        } else {
            buf.extend_from_slice(format!("\x1b_Gq=2,m={more};").as_bytes());
        }
        buf.extend_from_slice(chunk);
        buf.extend_from_slice(b"\x1b\\");
    }
}

pub use crate::graphics_legacy::Legacy;

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(extra: &str, pixels: &[u8], size: (u32, u32), cell: (u16, u16)) -> Graphics {
        let mut graphics = Graphics::default();
        let payload = String::from_utf8(encode(pixels)).unwrap();
        let packet = format!(
            "\x1b_Ga=T,i=1,f=24,s={},v={},C=1{extra};{payload}\x1b\\",
            size.0, size.1
        );
        let out = graphics.command(packet.as_bytes(), (0, 0), cell);
        assert_eq!(out.replies, b"\x1b_Gi=1;OK\x1b\\");
        graphics
    }

    #[test]
    fn cached_prepare_reuses_tiles_without_reupload_for_still_panes() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        // No image traffic: no source walk, no tiles.
        host.begin();
        assert!(host
            .prepare_cached(7, None, &g, (0, 0, 1, 1), (1, 1))
            .is_empty());
        host.finish();
        // First prepare uploads once and populates the cache.
        host.begin();
        let first = host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(first.len(), 1);
        assert!(!host.pending.is_empty());
        assert!(!host.prepare_cache.is_empty());
        // Same token, same geometry: cached tiles, same host id, no new upload.
        host.begin();
        let cached = host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(cached, first);
        assert!(host.pending.is_empty());
        // Token change: full re-prepare; tile rect is unchanged so the list
        // matches (upload cadence is owned by the texture timestamp).
        host.begin();
        let changed = host.prepare_cached(7, Some(4), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(changed, first);
        // Geometry change with the same token: cache miss by key, same id.
        host.begin();
        let moved = host.prepare_cached(7, Some(4), &g, (1, 0, 1, 1), (1, 1));
        host.finish();
        assert!(moved.is_empty());
        // A different pane id must not alias the first pane's tiles.
        host.begin();
        let other = host.prepare_cached(9, Some(4), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_ne!(other[0].id, first[0].id);
    }

    #[test]
    fn hidden_textures_are_retained_and_reveal_is_free() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        let first = host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(first.len(), 1);
        // Hide the image for an epoch: the texture is retained (it is the
        // hidden-pane cache), so no delete goes to the host, but the cheap
        // tile list is culled (it is rebuilt in microseconds on reveal).
        host.begin();
        host.finish();
        assert!(host.pending.is_empty());
        assert!(host.prepare_cache.is_empty());
        assert_eq!(host.bytes, 4);
        // Revealing again reuses the retained texture: same host id, zero
        // image bytes on the wire. This is what makes strip cycling with an
        // image-heavy pane cheap.
        host.begin();
        let visible = host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(visible[0].id, first[0].id);
        assert!(host.pending.is_empty());
        // `clear()` drops textures and cache entries together.
        assert!(!host.clear().is_empty());
        assert!(host.prepare_cache.is_empty());
    }

    #[test]
    fn crop_scale_and_cell_padding_are_applied_locally() {
        let g = scene(
            ",x=1,w=1,c=2,r=1,X=1",
            &[255, 0, 0, 0, 255, 0],
            (2, 1),
            (2, 2),
        );
        let pixels = raster(g.source(1).unwrap(), &g.placements()[0], 0, 0, 4, 2);
        for row in pixels.chunks_exact(16) {
            assert_eq!(&row[..4], &[0, 0, 0, 0]);
            assert_eq!(&row[4..], &[0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255]);
        }
    }

    #[test]
    fn bilinear_rgb_scaling_has_exact_endpoints_and_opaque_alpha() {
        let g = scene(",c=4,r=1", &[0, 0, 0, 255, 255, 255], (2, 1), (1, 1));
        assert_eq!(
            raster(g.source(1).unwrap(), &g.placements()[0], 0, 0, 4, 1),
            [0, 0, 0, 255, 64, 64, 64, 255, 191, 191, 191, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn overlapping_child_ids_are_isolated_and_only_owned_ids_are_deleted() {
        let g = scene("", &[255, 0, 0], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        let first = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        let second = host.prepare(2, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_ne!(first[0].id, second[0].id);
        host.begin();
        let again = host.prepare(2, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(again[0].id, second[0].id);
        // Pane 1's texture is hidden, not gone: retained under budget so a
        // reveal costs nothing. `clear()` deletes both, each exactly once.
        assert!(host.pending.is_empty());
        let cleared = String::from_utf8(host.clear()).unwrap();
        for id in [first[0].id, second[0].id] {
            assert_eq!(cleared.matches(&format!("i={id},")).count(), 1);
        }
        assert_eq!(host.bytes, 0);
        assert!(host.clear().is_empty());
    }

    #[test]
    fn equal_depth_orders_by_child_image_id_not_transmission_order() {
        let mut g = Graphics::default();
        for (id, pixels) in [(2, "AP8A"), (1, "/wAA")] {
            let packet = format!("\x1b_Ga=T,i={id},f=24,s=1,v=1,C=1;{pixels}\x1b\\");
            assert!(!g.command(packet.as_bytes(), (0, 0), (1, 1)).unsupported);
        }
        let mut host = Host::default();
        host.begin();
        let tiles = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        assert_eq!(tiles.len(), 2);
        let id_for = |image: u64| {
            host.textures
                .iter()
                .find(|(key, _)| key[2] == image)
                .unwrap()
                .1
                .id
        };
        assert_eq!(tiles[0].id, id_for(1));
        assert_eq!(tiles[1].id, id_for(2));
        let mut output = Cell::default();
        for tile in &tiles {
            output = tile.cell(0, 0, output).unwrap();
        }
        assert_eq!(child_id(output), id_for(2));
    }

    #[test]
    fn budget_pressure_reclaims_hidden_textures_and_reveal_reuploads_local_source() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let source = std::sync::Arc::clone(&g.source(1).unwrap().pixels);
        let mut host = Host::default();
        host.begin();
        let first = host.prepare(3, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        // Hiding alone releases nothing: the texture is the hidden-pane cache.
        host.begin();
        host.finish();
        assert_eq!(host.bytes, 4);
        assert!(host.pending.is_empty());
        // Budget pressure while hidden reclaims it: the local source pixels
        // stay owned by the pane, so a later reveal re-rasters and re-uploads
        // from them under a fresh host id.
        host.begin();
        host.bytes = MAX_HOST_BYTES;
        assert!(host.budget(4));
        host.bytes -= MAX_HOST_BYTES - 4;
        assert!(String::from_utf8_lossy(&host.pending).contains(&format!("i={}", first[0].id)));
        host.finish();
        assert_eq!(host.bytes, 0);
        host.begin();
        let visible = host.prepare(3, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_ne!(visible[0].id, first[0].id);
        assert!(host.pending.starts_with(b"\x1b_Ga=T,U=1"));
        assert!(std::sync::Arc::ptr_eq(
            &source,
            &g.source(1).unwrap().pixels
        ));
    }

    #[test]
    fn clipped_tiles_emit_explicit_coordinates_and_never_touch_outside_cells() {
        let g = scene(",c=300,r=2", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        let tiles = host.prepare(1, &g, (255, 1, 45, 1), (1, 1));
        assert_eq!(tiles.len(), 1);
        let t = &tiles[0];
        assert_eq!((t.col, t.row, t.cols, t.rows), (255, 1, 45, 1));
        assert!(t.cell(0, 255, Cell::default()).is_none());
        assert!(t.cell(1, 254, Cell::default()).is_none());
        assert!(t.cell(1, 300, Cell::default()).is_none());
        let c = t.cell(1, 299, Cell::default()).unwrap();
        assert_eq!(c.ch, PLACEHOLDER);
        assert_eq!(c.width, 1);
        assert_eq!(c.combining[0], DIACRITICS[0]);
        assert_eq!(c.combining[1], DIACRITICS[44]);
        assert_eq!(c.combining[2], DIACRITICS[(t.id >> 24) as usize]);
        assert_eq!(c.style.underline_color, CColor::Rgb(0, 0, 1));
        assert_eq!(child_id(c), t.id);
    }

    #[test]
    fn wide_scenes_split_before_diacritic_index_overflow() {
        let g = scene(",c=600,r=1", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        let tiles = host.prepare(1, &g, (0, 0, 600, 1), (1, 1));
        assert_eq!(tiles.iter().map(|t| t.cols as u32).sum::<u32>(), 600);
        assert!(tiles.iter().all(|t| t.cols <= 256 && t.rows <= 256));
        assert_eq!(tiles.len(), 3);
    }

    #[test]
    fn eviction_refresh_reuploads_without_changing_placeholder_ids() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        let first = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        host.begin();
        host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert!(host.pending.is_empty());
        for t in host.textures.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        assert!(host.refresh_due());
        host.begin();
        let refreshed = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(first[0].id, refreshed[0].id);
        assert!(!host.pending.is_empty());
        assert!(!host.refresh_due());
    }

    #[test]
    fn refresh_due_ignores_hidden_textures_and_cached_hits_still_reupload() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        for t in host.textures.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        // Visible and due: refresh. The dirty frame serves from the prepare
        // cache, which must not swallow the re-upload or `refresh_due` would
        // keep marking frames dirty forever (a repaint busy-loop).
        assert!(host.refresh_due());
        host.begin();
        host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert!(!host.pending.is_empty());
        assert!(!host.refresh_due());
        // Hidden and due: NOT a refresh. The texture cannot be re-uploaded
        // until its pane paints again, so flagging it would busy-loop.
        for t in host.textures.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        host.begin();
        host.finish();
        assert!(!host.refresh_due());
        // Reveal after the stale hide: the reveal frame itself stays free
        // (that is the point of retention); the heal is deferred and
        // scheduled through `refresh_due`, so a dropped image still recovers
        // within REVEAL_HEAL instead of blocking the navigation paint.
        host.begin();
        host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert!(host.pending.is_empty());
        assert!(!host.refresh_due());
        // Once the deferral elapses the heal fires on the next paint.
        for t in host.textures.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        assert!(host.refresh_due());
        host.begin();
        host.prepare_cached(7, Some(3), &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert!(!host.pending.is_empty());
        assert!(!host.refresh_due());
    }

    #[test]
    fn reveal_after_long_hide_ships_zero_bytes_and_schedules_the_heal() {
        let mut host = Host::default();
        let id = allocate();
        host.begin();
        assert!(host.show_legacy(1, id, 1, b"page", 8));
        host.finish();
        // Hidden for several frames; texture goes refresh-stale meanwhile.
        host.begin();
        host.finish();
        host.begin();
        host.finish();
        for t in host.legacy.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        // Reveal: zero image bytes, heal deferred (uploaded was pushed
        // forward so refresh_due stays quiet for REVEAL_HEAL).
        host.begin();
        assert!(host.show_legacy(1, id, 1, b"page", 8));
        host.finish();
        assert!(host.pending.is_empty());
        assert!(!host.refresh_due());
        // Deferral elapsed while still visible: the heal re-uploads.
        for t in host.legacy.values_mut() {
            t.uploaded = Instant::now() - REFRESH;
        }
        assert!(host.refresh_due());
        host.begin();
        assert!(host.show_legacy(1, id, 1, b"page", 8));
        host.finish();
        assert_eq!(host.pending, b"page");
        // New content on reveal still uploads immediately: a revision bump
        // is a page flip, not a heal, and must never show the old page.
        host.begin();
        host.finish();
        host.begin();
        host.finish();
        host.begin();
        assert!(host.show_legacy(1, id, 2, b"next", 8));
        host.finish();
        assert_eq!(host.pending, b"next");
    }

    #[test]
    fn budget_evicts_least_recently_painted_hidden_texture_first() {
        let mut host = Host::default();
        let (old, new) = (allocate(), allocate());
        // Pane 1 painted long ago, pane 2 more recently, both now hidden.
        host.begin();
        assert!(host.show_legacy(1, old, 1, b"old", MAX_HOST_BYTES / 2));
        host.begin();
        assert!(host.show_legacy(2, new, 1, b"new", MAX_HOST_BYTES / 2));
        host.begin();
        host.finish();
        assert_eq!(host.bytes, MAX_HOST_BYTES / 2 * 2);
        // A newcomer needing half the budget must evict pane 1's texture
        // (least recently painted), not pane 2's.
        host.begin();
        assert!(host.show_legacy(3, allocate(), 1, b"incoming", MAX_HOST_BYTES / 2));
        let pending = String::from_utf8_lossy(&host.pending);
        assert!(pending.contains(&format!("a=d,d=I,i={old}")));
        assert!(!pending.contains(&format!("a=d,d=I,i={new}")));
        assert!(host.legacy.contains_key(&new));
        assert!(!host.legacy.contains_key(&old));
    }

    #[test]
    fn painted_pane_that_dropped_a_texture_deletes_it_hidden_pane_keeps_it() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        let legacy_id = allocate();
        host.begin();
        let tiles = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        assert!(host.show_legacy(2, legacy_id, 1, b"page", 8));
        host.finish();
        // Pane 1 paints again without its image (child deleted it): its
        // texture is proven dead and freed. Pane 2 is hidden: retained.
        host.begin();
        let empty = Graphics::default();
        assert!(host.prepare(1, &empty, (0, 0, 1, 1), (1, 1)).is_empty());
        host.finish();
        let pending = String::from_utf8_lossy(&host.pending);
        assert!(pending.contains(&format!("a=d,d=I,i={}", tiles[0].id)));
        assert!(!pending.contains(&format!("a=d,d=I,i={legacy_id}")));
        assert!(host.legacy.contains_key(&legacy_id));
        assert_eq!(host.bytes, 8);
    }

    #[test]
    fn retain_panes_frees_closed_panes_and_returns_the_deletes() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        let legacy_id = allocate();
        host.begin();
        let tiles = host.prepare_cached(1, Some(1), &g, (0, 0, 1, 1), (1, 1));
        assert!(host.show_legacy(2, legacy_id, 1, b"page", 8));
        host.finish();
        // Pane 1 closed; pane 2 lives on.
        let live = std::collections::HashSet::from([2u64]);
        let deletes = String::from_utf8(host.retain_panes(&live)).unwrap();
        assert!(deletes.contains(&format!("a=d,d=I,i={}", tiles[0].id)));
        assert!(!deletes.contains(&format!("a=d,d=I,i={legacy_id}")));
        assert!(host.textures.is_empty());
        assert!(host.prepare_cache.is_empty());
        assert!(host.legacy.contains_key(&legacy_id));
        assert_eq!(host.bytes, 8);
        // No-op when nothing closed.
        assert!(host.retain_panes(&live).is_empty());
    }

    #[test]
    fn stale_budget_is_reclaimed_before_replacement_admission() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        let old = allocate();
        host.bytes = MAX_HOST_BYTES;
        host.textures.insert(
            vec![999],
            Texture {
                id: old,
                epoch: 0,
                uploaded: Instant::now(),
                bytes: MAX_HOST_BYTES,
            },
        );
        host.begin();
        let tiles = host.prepare(1, &g, (0, 0, 1, 1), (1, 1));
        host.finish();
        assert_eq!(tiles.len(), 1);
        assert_eq!(host.bytes, 4);
        assert!(host
            .pending
            .starts_with(format!("\x1b_Ga=d,d=I,i={old},q=2\x1b\\").as_bytes()));
    }

    #[test]
    fn native_and_legacy_share_one_live_host_budget() {
        let g = scene("", &[1, 2, 3], (1, 1), (1, 1));
        let mut host = Host::default();
        host.begin();
        assert!(host.show_legacy(1, allocate(), 1, b"upload", MAX_HOST_BYTES));
        assert!(host.prepare(1, &g, (0, 0, 1, 1), (1, 1)).is_empty());
        assert_eq!(host.bytes, MAX_HOST_BYTES);
        host.begin();
        assert_eq!(host.prepare(1, &g, (0, 0, 1, 1), (1, 1)).len(), 1);
        host.finish();
        assert_eq!(host.bytes, 4);
    }

    #[test]
    fn virtual_placement_revision_and_footprint_changes_upload_immediately() {
        let mut host = Host::default();
        let id = allocate();
        host.begin();
        assert!(host.show_legacy(1, id, 1, b"one", 4));
        host.finish();
        assert_eq!(host.pending, b"one");
        host.begin();
        assert!(host.show_legacy(1, id, 1, b"one", 4));
        host.finish();
        assert!(host.pending.is_empty());
        host.begin();
        assert!(host.show_legacy(1, id, 2, b"two", 4));
        host.finish();
        assert_eq!(host.pending, b"two");
        host.begin();
        assert!(host.show_legacy(1, id, 3, b"three", 8));
        host.finish();
        assert!(host.pending.ends_with(b"three"));
        assert_eq!(host.bytes, 8);
    }

    #[test]
    fn canonical_upload_chunks_are_bounded_quiet_and_direct() {
        let mut bytes = Vec::new();
        let rgba = vec![123; 10000];
        upload(&mut bytes, allocate(), &rgba, 50, 50, 10, 10);
        let text = String::from_utf8(bytes).unwrap();
        let packets: Vec<_> = text.split("\x1b\\").filter(|p| !p.is_empty()).collect();
        // A redundant raster compresses far below one chunk, so the whole
        // transfer is a single final packet rather than the four base64
        // chunks the uncompressed payload needed.
        assert_eq!(packets.len(), 1);
        let last = packets.len() - 1;
        for (n, packet) in packets.iter().enumerate() {
            let (header, payload) = packet.split_once(';').unwrap();
            assert!(header.contains("q=2"));
            assert!(header.contains(if n == last { "m=0" } else { "m=1" }));
            assert!(payload.len() <= 4096);
            assert_eq!(payload.len() % 4, 0);
            assert!(!header.contains("t=f") && !header.contains("t=s"));
        }
        // The payload is advertised as zlib and inflates back to the exact
        // raster, at its full uncompressed pixel dimensions.
        let header = packets[0].split_once(';').unwrap().0;
        assert!(header.contains(",o=z"), "{header}");
        assert!(header.contains("s=50,v=50"), "{header}");
        let payload: String = packets
            .iter()
            .map(|p| p.split_once(';').unwrap().1)
            .collect();
        let raw = decode_base64(payload.as_bytes());
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec_zlib(&raw).unwrap(),
            rgba
        );
    }

    fn decode_base64(input: &[u8]) -> Vec<u8> {
        const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        for quad in input.chunks(4) {
            let mut bits = 0u32;
            let mut n = 0;
            for &b in quad {
                if b == b'=' {
                    bits <<= 6;
                    continue;
                }
                bits = (bits << 6) | ABC.iter().position(|&c| c == b).unwrap() as u32;
                n += 1;
            }
            for i in 0..n - 1 {
                out.push((bits >> (16 - i * 8)) as u8);
            }
        }
        out
    }
}

#[cfg(test)]
mod page_cost {
    use super::*;

    /// A full-pane page raster must not cost megabytes of host output per
    /// redraw. This is the regression guard for image-heavy panes making the
    /// whole UI sluggish: every pane is painted by one thread, so one pane's
    /// upload volume is the whole app's frame time.
    #[test]
    fn full_pane_page_upload_stays_small_enough_to_stream_per_frame() {
        let (w, h) = (1400u32, 1000u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            if (y / 4) % 6 < 2 {
                for x in 0..w {
                    if (x * 7 + y * 13) % 11 < 4 {
                        let i = ((y * w + x) * 4) as usize;
                        rgba[i] = 20;
                        rgba[i + 1] = 20;
                        rgba[i + 2] = 20;
                    }
                }
            }
        }
        let mut buf = Vec::new();
        let start = Instant::now();
        upload(&mut buf, allocate(), &rgba, w, h, 140, 50);
        let elapsed = start.elapsed();
        // Uncompressed base64 of this raster is ~7.4 MB.
        assert!(
            buf.len() < 256 * 1024,
            "{} bytes is too much host output for one page",
            buf.len()
        );
        assert!(
            elapsed < Duration::from_millis(60),
            "{elapsed:?} per page upload would drop frames"
        );
    }
}
