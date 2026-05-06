//! Font registry + per-(name, size) atlas cache.
//!
//! JS code refers to fonts by name (`font-family`). The registry maps
//! a name to its TTF byte buffer. Each unique `(name, px_size)` pair
//! used by the UI gets a fontdue-rasterised A8 atlas, uploaded once
//! to the texture pool and cached. Paint emits per-glyph
//! `COPY_RECT`s referencing the cached texture handle.
//!
//! N4 ships an embedded **default** font (Noto Sans Regular,
//! 27 KB, SIL OFL). `1fpga:gui.registerFont(name, ttfBytes)` (added
//! in the host module) registers additional fonts at runtime.

use std::collections::{HashMap, HashSet};

use menu_core_host::device::Device;
use menu_core_host::error::DeviceError;
use menu_core_host::protocol::TextureFormat;
use menu_core_host::texture::{TextureHandle, TextureSpec};

use crate::font::atlas::{AtlasError, FontAtlas, build_atlas};

pub mod atlas;

/// Default font bundled into the binary. Other fonts can be registered
/// via the host API at runtime.
pub const DEFAULT_FONT_NAME: &str = "default";
const DEFAULT_FONT_BYTES: &[u8] =
    include_bytes!("../../../menu-core/fonts/NotoSans-Regular.ttf");

/// Always include `'?'` as the missing-glyph fallback so unknown
/// codepoints render as a question mark instead of disappearing.
const FALLBACK_CHAR: char = '?';

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("font '{0}' not registered")]
    UnknownFont(String),
    #[error("atlas build failed: {0}")]
    Atlas(#[from] AtlasError),
    #[error("device error: {0}")]
    Device(#[from] DeviceError),
}

/// Cache key. Px size is rounded to integer — rasterising at 23.7 vs
/// 24.0 px produces visually-identical glyphs and shares the atlas.
pub type AtlasKey = (String, u16);

pub struct CachedAtlas {
    pub atlas: FontAtlas,
    pub texture: TextureHandle,
}

pub struct FontRegistry {
    fonts: HashMap<String, Vec<u8>>,
    atlases: HashMap<AtlasKey, CachedAtlas>,
}

impl Default for FontRegistry {
    fn default() -> Self {
        let mut fonts = HashMap::new();
        fonts.insert(DEFAULT_FONT_NAME.to_string(), DEFAULT_FONT_BYTES.to_vec());
        Self {
            fonts,
            atlases: HashMap::new(),
        }
    }
}

impl FontRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a font's TTF bytes under `name`.
    pub fn register(&mut self, name: impl Into<String>, ttf: Vec<u8>) {
        self.fonts.insert(name.into(), ttf);
    }

    /// Return `true` if `name` is registered (either bundled or via
    /// `register`).
    pub fn has(&self, name: &str) -> bool {
        self.fonts.contains_key(name)
    }

    /// Ensure the atlas for `(name, px_size)` covers every character
    /// in `required` (plus `'?'` for the missing-glyph fallback). The
    /// atlas is built or rebuilt on demand: if the cached version
    /// already contains all required chars, returns it; otherwise
    /// rasterises a fresh atlas with the union of old + new chars and
    /// uploads it as a new texture. The previous texture is left in
    /// the pool (eviction is future work).
    pub fn ensure(
        &mut self,
        device: &mut Device,
        name: &str,
        px_size: u16,
        required: &HashSet<char>,
    ) -> Result<&CachedAtlas, FontError> {
        let key: AtlasKey = (name.to_string(), px_size);

        // Compute the full charset we need: required ∪ {'?'} ∪
        // anything the existing atlas already has (so we don't lose
        // glyphs by rebuilding).
        let mut chars: HashSet<char> = required.clone();
        chars.insert(FALLBACK_CHAR);
        let needs_build = match self.atlases.get(&key) {
            None => true,
            Some(cached) => required.iter().any(|c| !cached.atlas.has_glyph(*c)),
        };

        if needs_build {
            // When rebuilding, preserve any chars the existing atlas
            // already had so previously-cached PendingRenders don't
            // suddenly miss glyphs they expected.
            if let Some(prev) = self.atlases.get(&key) {
                // Copy the existing glyphs' chars into `chars` —
                // FontAtlas exposes them via `glyph()` lookups; here
                // we iterate the printable ASCII range as a cheap
                // approximation. (Full enumeration via iter_glyphs
                // would be cleaner; left as a future polish.)
                for ch in '\u{0020}'..='\u{007E}' {
                    if prev.atlas.has_glyph(ch) {
                        chars.insert(ch);
                    }
                }
            }
            let bytes = self
                .fonts
                .get(name)
                .ok_or_else(|| FontError::UnknownFont(name.to_string()))?;
            let charset: String = chars.iter().collect();
            let atlas = build_atlas(bytes, px_size as f32, &charset, 1024, 4096)?;
            let texture = device.upload_texture(&TextureSpec {
                format: TextureFormat::A8,
                width: atlas.width,
                height: atlas.height,
                stride: atlas.width as u32,
                data: &atlas.bytes,
            })?;
            tracing::info!(
                "fontatlas: {} @ {} px — {}x{}, {} glyphs, tex_id={}",
                name,
                px_size,
                atlas.width,
                atlas.height,
                chars.len(),
                texture.id
            );
            self.atlases
                .insert(key.clone(), CachedAtlas { atlas, texture });
        }
        Ok(self
            .atlases
            .get(&key)
            .expect("inserted just above or already present"))
    }

    /// Lookup without building. Returns `None` if `(name, px_size)`
    /// hasn't been built yet.
    pub fn get(&self, name: &str, px_size: u16) -> Option<&CachedAtlas> {
        self.atlases.get(&(name.to_string(), px_size))
    }
}
