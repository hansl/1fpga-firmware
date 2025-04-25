use boa_engine::{js_string, Context, JsResult, JsString, Module};
use boa_interop::{IntoJsFunctionCopied, IntoJsModule};

fn shutdown_() -> JsResult<()> {
    #[cfg(target_os = "linux")]
    unsafe {
        nix::libc::reboot(nix::libc::RB_POWER_OFF);
    }

    Ok(())
}

pub fn create_module(context: &mut Context) -> JsResult<(JsString, Module)> {
    Ok((
        js_string!("system"),
        [(
            js_string!("shutdown"),
            shutdown_.into_js_function_copied(context),
        )]
        .into_js_module(context),
    ))
}
