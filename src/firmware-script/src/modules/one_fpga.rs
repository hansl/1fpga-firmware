use std::rc::Rc;

use boa_engine::{js_string, Context, JsResult, JsString, Module};
use boa_macros::js_str;

use crate::module_loader::OneFpgaModuleLoader;

mod commands;
mod controllers;
mod core;
mod db;
mod dom;
mod fs;
mod gui;
mod net;
mod osd;
mod settings;
mod system;
mod upgrade;
mod utils;
mod video;

mod globals;

pub use commands::CommandMap;
pub use globals::classes::*;

pub(super) fn register_modules(
    loader: Rc<OneFpgaModuleLoader>,
    context: &mut Context,
) -> JsResult<()> {
    let modules = [
        commands::create_module,
        controllers::create_module,
        core::create_module,
        db::create_module,
        dom::create_module,
        fs::create_module,
        gui::create_module,
        net::create_module,
        settings::create_module,
        system::create_module,
        osd::create_module,
        upgrade::create_module,
        utils::create_module,
        video::create_module,
    ];

    for create_fn in modules.iter() {
        let (name, module) = create_fn(context)?;
        let module_name = JsString::concat(js_str!("1fpga:"), name.as_str());
        loader.insert_named(module_name, module);
    }

    // The patrons module.
    loader.insert_named(
        js_string!("1fpga:patrons"),
        Module::parse_json(
            js_string!(include_str!("../../../../scripts/patreon/patrons.json")),
            context,
        )?,
    );

    globals::register_globals(context)?;

    Ok(())
}
