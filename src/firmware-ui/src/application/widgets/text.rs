use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Dimensions;
use embedded_graphics::pixelcolor::PixelColor;
use embedded_graphics::prelude::Point;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::transform::Transform;
use embedded_graphics::Drawable;
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use u8g2_fonts::{Font, FontRenderer};

#[derive(Clone)]
pub struct FontRendererView<'s, C: PixelColor> {
    pub top_left: Point,
    pub vertical_position: VerticalPosition,
    pub alignment: HorizontalAlignment,
    pub text: &'s str,
    pub renderer: FontRenderer,
    pub color: C,
}

impl<'s, C: PixelColor> FontRendererView<'s, C> {
    pub fn new<F: Font>(
        color: C,
        vertical_position: VerticalPosition,
        alignment: HorizontalAlignment,
        text: &'s str,
    ) -> Self {
        let top_left = Point::zero();
        Self {
            top_left,
            vertical_position,
            alignment,
            text,
            renderer: FontRenderer::new::<F>(),
            color,
        }
    }
}

impl<C: PixelColor> Dimensions for FontRendererView<'_, C> {
    fn bounding_box(&self) -> Rectangle {
        self.renderer
            .get_rendered_dimensions(self.text, self.top_left, self.vertical_position)
            .unwrap()
            .bounding_box
            .unwrap_or_default()
    }
}

impl<C: PixelColor> Transform for FontRendererView<'_, C> {
    fn translate(&self, by: Point) -> Self {
        Self {
            top_left: self.top_left + by,
            renderer: self.renderer.clone(),
            ..*self
        }
    }

    fn translate_mut(&mut self, by: Point) -> &mut Self {
        self.top_left += by;
        self
    }
}

impl<C: PixelColor> Drawable for FontRendererView<'_, C> {
    type Color = C;
    type Output = ();

    fn draw<D>(&self, target: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let position = match self.alignment {
            HorizontalAlignment::Left => self.top_left,
            HorizontalAlignment::Center => {
                let width = self.bounding_box().size.width as i32;
                let text_width = self
                    .renderer
                    .get_rendered_dimensions(self.text, self.top_left, VerticalPosition::Baseline)
                    .expect("Could not get rendered dimensions")
                    .bounding_box
                    .expect("No bounding box")
                    .size
                    .width as i32;
                let x = self.top_left.x + width - (text_width / 2);
                Point::new(x, self.top_left.y)
            }
            HorizontalAlignment::Right => {
                let width = self.bounding_box().size.width as i32;
                let text_width = self
                    .renderer
                    .get_rendered_dimensions(self.text, self.top_left, VerticalPosition::Baseline)
                    .expect("Could not get rendered dimensions")
                    .bounding_box
                    .expect("No bounding box")
                    .size
                    .width as i32;
                let x = self.top_left.x + width - text_width;
                Point::new(x, self.top_left.y)
            }
        };

        self.renderer
            .render(
                self.text,
                position,
                VerticalPosition::Baseline,
                FontColor::Transparent(self.color),
                target,
            )
            .map_err(|e| match e {
                u8g2_fonts::Error::DisplayError(e) => e,
                u8g2_fonts::Error::BackgroundColorNotSupported => unreachable!(),
                u8g2_fonts::Error::GlyphNotFound(c) => unreachable!("{}", c),
            })?;
        Ok(())
    }
}
