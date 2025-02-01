use crate::console::TracingLogger;
use crate::module_loader::OneFpgaModuleLoader;
use crate::modules::CommandMap;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsObject, JsResult, JsValue, Module, Source};
use boa_macros::{js_str, Finalize, JsData, Trace};
use boa_runtime::RegisterOptions;
use firmware_ui::application::OneFpgaApp;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use tracing::{debug, error, info};

mod module_loader;

mod commands;
mod console;
mod modules;

/// The application type for HostDefined information.
#[derive(Clone, Trace, Finalize, JsData)]
pub(crate) struct HostData {
    // TODO: remove the pointer. This is safe because the JS code
    //       stops execution before the App is dropped, but it would
    //       be better to have a safe way to handle this.
    //       A RefCell isn't good enough because it's recursive.
    /// The 1FPGA application.
    #[unsafe_ignore_trace]
    app: Rc<*mut OneFpgaApp>,

    /// A command map that needs to be shared.
    #[unsafe_ignore_trace]
    command_map: Rc<*mut CommandMap>,
}

impl std::fmt::Debug for HostData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostData").finish()
    }
}

impl HostData {
    pub fn app(&self) -> &OneFpgaApp {
        unsafe { self.app.as_ref().as_ref().unwrap() }
    }

    pub fn app_mut(&self) -> &mut OneFpgaApp {
        unsafe { self.app.as_mut().unwrap() }
    }

    pub fn command_map_mut(&self) -> &mut CommandMap {
        unsafe { self.command_map.as_mut().unwrap() }
    }
}

fn create_context(
    script: Option<&impl AsRef<Path>>,
    host_defined: HostData,
) -> JsResult<(Context, Rc<OneFpgaModuleLoader>)> {
    let loader = match script {
        Some(p) => {
            let dir = p.as_ref().parent().expect("Cannot use root.");

            Rc::new(OneFpgaModuleLoader::new(dir).expect("Could not find the script folder."))
        }
        None => Rc::new(OneFpgaModuleLoader::default()),
    };

    let mut context = Context::builder().module_loader(loader.clone()).build()?;
    context.insert_data(host_defined);

    let version = {
        let major = (env!("CARGO_PKG_VERSION_MAJOR"))
            .parse::<u32>()
            .expect("Invalid major version");
        let minor = (env!("CARGO_PKG_VERSION_MINOR"))
            .parse::<u32>()
            .expect("Invalid major version");
        let patch = (env!("CARGO_PKG_VERSION_PATCH"))
            .parse::<u32>()
            .expect("Invalid major version");

        let version = JsObject::with_null_proto();
        version.set(js_str!("major"), major, false, &mut context)?;
        version.set(js_str!("minor"), minor, false, &mut context)?;
        version.set(js_str!("patch"), patch, false, &mut context)?;
        version
    };

    let one_fpga = JsObject::default();
    one_fpga.set(js_str!("name"), js_string!("1FPGA"), false, &mut context)?;
    one_fpga.set(js_str!("version"), version, false, &mut context)?;

    context.register_global_property(js_str!("ONE_FPGA"), one_fpga, Attribute::ENUMERABLE)?;

    Ok((context, loader))
}

pub fn run(
    script: Option<&impl AsRef<Path>>,
    mut app: OneFpgaApp,
) -> Result<(), Box<dyn std::error::Error>> {
    app.init_platform();
    let app = Rc::new((&mut app) as *mut OneFpgaApp);
    let mut command_map = CommandMap::default();
    let host_defined = HostData {
        app,
        command_map: Rc::new(&mut command_map as *mut CommandMap),
    };

    debug!("Loading JavaScript...");
    let start = Instant::now();

    let (mut context, loader) = create_context(script, host_defined)?;
    boa_runtime::register(
        &mut context,
        RegisterOptions::new().with_console_logger(TracingLogger),
    )?;

    modules::register_modules(loader.clone(), &mut context)?;
    debug!("Context created in {}ms.", start.elapsed().as_millis());

    let start = Instant::now();
    let module = match script {
        Some(script_path) => {
            let source = Source::from_reader(
                std::fs::File::open(script_path)?,
                Some(script_path.as_ref()),
            );

            Module::parse(source, None, &mut context)?
        }
        None => {
            let source = Source::from_bytes(b"export { main } from '/main.js';");
            Module::parse(source, None, &mut context)?
        }
    };

    debug!("Script parsed in {}ms.", start.elapsed().as_millis());

    let start = Instant::now();
    if let Err(e) = module
        .load_link_evaluate(&mut context)
        .await_blocking(&mut context)
    {
        if let Ok(e) = e.try_native(&mut context) {
            error!(error = ?e, "Native error");
        } else {
            error!(error = ?e, "Error loading script");
        }

        return Err(e.into());
    }
    debug!(
        "Script loaded and evaluated in {}ms.",
        start.elapsed().as_millis()
    );

    let start = Instant::now();

    debug!("Script loaded in {}ms.", start.elapsed().as_millis());
    let start = Instant::now();
    let main_fn = module
        .namespace(&mut context)
        .get(js_string!("main"), &mut context)?;

    let mut result = main_fn.as_callable().expect("Main was not callable").call(
        &JsValue::undefined(),
        &[],
        &mut context,
    )?;

    // Loop until the promise chain is resolved.
    while let Some(p) = result.as_promise() {
        match p.await_blocking(&mut context) {
            Ok(v) => {
                // If `v` is not a promise this will simply break the `while`.
                result = v;
            }
            Err(e) => {
                error!("Javascript Error: {}", e);
                return Err(e.try_native(&mut context)?.into());
            }
        }
    }

    debug!("Main executed in {}ms.", start.elapsed().as_millis());
    info!(?result, "Script executed successfully.");
    Ok(())
}
