#![cfg(feature = "platform_desktop")]

use crate::macguiver::buffer::DrawBuffer;
use crate::macguiver::platform::sdl::{
    BinaryColorTheme, OutputSettingsBuilder, SdlInitState, SdlPlatform, Window,
};
use crate::macguiver::platform::{Platform, PlatformWindow};
use crate::main_inner::Flags;
use crate::platform::{sizes, Core, CoreManager, MiSTerPlatform};
use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::BinaryColor;
use sdl3::event::Event;
use std::path::Path;
use tracing::info;

pub struct DummyCore;

impl Core for DummyCore {
    fn send_key(&mut self, key: u8) {
        info!("DummyCore::send_key({})", key);
    }
}

pub struct DummyCoreManager;

impl CoreManager for DummyCoreManager {
    type Core = DummyCore;

    fn load_program(&mut self, path: impl AsRef<Path>) -> Result<Self::Core, String> {
        info!("DummyCoreManager::load_program({:?})", path.as_ref());
        Ok(DummyCore)
    }

    fn show_menu(&mut self) {}

    fn hide_menu(&mut self) {}
}

pub struct DesktopPlatform {
    platform: SdlPlatform<BinaryColor>,
    window_title: Window<BinaryColor>,
    window_main: Window<BinaryColor>,
    core_manager: DummyCoreManager,
}

impl Default for DesktopPlatform {
    fn default() -> Self {
        let mut platform = SdlPlatform::init(SdlInitState::new(
            OutputSettingsBuilder::new()
                .scale(3)
                .theme(BinaryColorTheme::LcdBlue)
                .build(),
        ));
        let mut window_title = platform.window("Title", sizes::TITLE);
        let mut window_main = platform.window("Title", sizes::MAIN);

        // Move the title above the window and make sure both are visible.
        let mut pos = window_main.position();
        pos.y -= sizes::TITLE.height as i32 + 128;
        window_title.set_position(pos);
        window_title.focus();
        window_main.focus();

        Self {
            platform,
            window_title,
            window_main,
            core_manager: DummyCoreManager,
        }
    }
}

impl MiSTerPlatform for DesktopPlatform {
    type Color = BinaryColor;
    type CoreManager = DummyCoreManager;

    fn init(&mut self, _: &Flags) {}

    fn update_toolbar(&mut self, buffer: &DrawBuffer<Self::Color>) {
        self.window_title.update(buffer);
    }
    fn update_main(&mut self, buffer: &DrawBuffer<Self::Color>) {
        self.window_main.update(buffer);
    }

    fn toolbar_dimensions(&self) -> Size {
        sizes::TITLE
    }
    fn main_dimensions(&self) -> Size {
        sizes::MAIN
    }

    fn events(&mut self) -> Vec<Event> {
        self.platform.events()
    }

    fn start_loop(&mut self) {}

    fn end_loop(&mut self) {}

    fn core_manager_mut(&mut self) -> &mut Self::CoreManager {
        &mut self.core_manager
    }
}