use boa_engine::{js_error, Context, JsError, JsResult, JsValue};
use boa_macros::{Finalize, JsData, Trace};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
use std::str::FromStr;
use u8g2_fonts::FontRenderer;

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) enum FontType {
    #[default]
    None,
    Monospace,
    SansSerif,
}

impl FromStr for FontType {
    type Err = JsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(Self::None),
            "monospace" => Ok(FontType::Monospace),
            "sans" | "sans-serif" => Ok(FontType::SansSerif),
            _ => Err(js_error!(Error: "Unknown font type {}", s)),
        }
    }
}

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) enum FontSize {
    #[default]
    None,
    Small,
    Medium,
    Large,
}

impl FromStr for FontSize {
    type Err = JsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(FontSize::None),
            "small" => Ok(FontSize::Small),
            "medium" => Ok(FontSize::Medium),
            "large" => Ok(FontSize::Large),
            _ => Err(js_error!(Error: "Unknown font size {}", s)),
        }
    }
}

#[derive(Default, Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) struct FontProp(FontType, FontSize);

impl FontProp {
    pub fn renderer(&self) -> FontRenderer {
        use u8g2_fonts::fonts::*;
        let font_type = if self.0 == FontType::None {
            FontType::SansSerif
        } else {
            self.0
        };
        let font_size = if self.1 == FontSize::None {
            FontSize::Medium
        } else {
            self.1
        };

        match (font_type, font_size) {
            (FontType::Monospace, FontSize::Small) => {
                FontRenderer::new::<u8g2_font_boutique_bitmap_7x7_t_all>()
            }
            (FontType::Monospace, FontSize::Medium) => {
                FontRenderer::new::<u8g2_font_boutique_bitmap_9x9_t_all>()
            }
            (FontType::Monospace, FontSize::Large) => {
                FontRenderer::new::<u8g2_font_unifont_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Small) => {
                FontRenderer::new::<u8g2_font_wqy12_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Medium) => {
                FontRenderer::new::<u8g2_font_wqy14_t_chinese3>()
            }
            (FontType::SansSerif, FontSize::Large) => {
                FontRenderer::new::<u8g2_font_wqy16_t_chinese3>()
            }
            (FontType::None, _) | (_, FontSize::None) => unsafe {
                // SAFETY: Verification done above.
                std::hint::unreachable_unchecked()
            },
        }
    }

    pub fn inherits(&self, other: FontProp) -> Self {
        match (self.0, self.1) {
            (FontType::None, FontSize::None) => other,
            (FontType::None, _) => Self(other.0, self.1),
            (_, FontSize::None) => Self(self.0, other.1),
            _ => *self,
        }
    }
}

impl boa_engine::value::TryFromJs for FontProp {
    fn try_from_js(value: &JsValue, context: &mut Context) -> JsResult<Self> {
        Self::from_str(value.to_string(context)?.to_std_string_lossy().as_str())
    }
}

impl FromStr for FontProp {
    type Err = JsError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((first, second)) = s.split_once(',') {
            Ok(Self(
                FontType::from_str(first)?,
                FontSize::from_str(second)?,
            ))
        } else if s.is_empty() || s == "default" {
            Ok(Self(FontType::None, FontSize::None))
        } else {
            if let Ok(ty) = FontType::from_str(s) {
                Ok(Self(ty, FontSize::None))
            } else if let Ok(sz) = FontSize::from_str(s) {
                Ok(Self(FontType::None, sz))
            } else {
                Err(js_error!(Error: "Unknown font specification {}", s))
            }
        }
    }
}

#[derive(Trace, Finalize, Debug, Clone, JsData)]
pub(crate) struct RenderingContextRef(#[unsafe_ignore_trace] Rc<RefCell<RenderingContext>>);

impl RenderingContextRef {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(RenderingContext::default())))
    }

    pub fn borrow(&self) -> Ref<RenderingContext> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<RenderingContext> {
        self.0.borrow_mut()
    }
}

#[derive(Default, Clone, Debug)]
pub(crate) struct FontRendererCache(HashMap<FontProp, FontRenderer>);

impl FontRendererCache {
    pub fn get_or_create(&mut self, spec: FontProp) -> &FontRenderer {
        self.0.entry(spec).or_insert(spec.renderer())
    }

    pub fn get(&self, spec: &FontProp) -> Option<&FontRenderer> {
        self.0.get(spec)
    }
}

#[derive(Default, Debug)]
pub(crate) struct RenderingContext {
    pub(crate) fonts: FontRendererCache,
}

impl RenderingContext {
    pub(crate) fn into_ref(self) -> RenderingContextRef {
        RenderingContextRef(Rc::new(RefCell::new(self)))
    }
}
