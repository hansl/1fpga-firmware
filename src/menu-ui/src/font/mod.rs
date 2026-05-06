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

use std::collections::HashMap;

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

/// ASCII-printable charset (space..tilde). Sized for a typical
/// English-locale menu; extension to wider charsets is easy via
/// re-rasterising at request time.
const DEFAULT_CHARSET_BYTES: std::ops::RangeInclusive<u8> = b' '..=b'~';

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

    /// Get the cached atlas + texture for `(name, px_size)`, building
    /// + uploading lazily on first call. Subsequent calls with the
    /// same key are cheap hashmap lookups.
    pub fn ensure(
        &mut self,
        device: &mut Device,
        name: &str,
        px_size: u16,
    ) -> Result<&CachedAtlas, FontError> {
        let key: AtlasKey = (name.to_string(), px_size);
        if !self.atlases.contains_key(&key) {
            let bytes = self
                .fonts
                .get(name)
                .ok_or_else(|| FontError::UnknownFont(name.to_string()))?;
            let charset: String = DEFAULT_CHARSET_BYTES.map(|b| b as char).collect();
            let atlas = build_atlas(bytes, px_size as f32, &charset, 512, 1024)?;
            let texture = device.upload_texture(&TextureSpec {
                format: TextureFormat::A8,
                width: atlas.width,
                height: atlas.height,
                stride: atlas.width as u32,
                data: &atlas.bytes,
            })?;
            tracing::info!(
                "fontatlas: {} @ {} px — {}x{}, tex_id={}",
                name,
                px_size,
                atlas.width,
                atlas.height,
                texture.id
            );
            self.atlases.insert(key.clone(), CachedAtlas { atlas, texture });
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
