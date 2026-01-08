use boa_engine::{js_string, Context, JsResult, JsString, Module};
use boa_macros::boa_module;

#[boa_module]
#[boa(rename_all = "camelCase")]
mod js {
    use super::super::osd::UiMenuOptions;
    use crate::AppRef;
    use boa_engine::interop::ContextData;
    use boa_engine::object::builtins::JsPromise;
    use boa_engine::Context;

    fn text_menu(
        mut options: UiMenuOptions,
        ContextData(mut app): ContextData<AppRef>,
        context: &mut Context,
    ) -> JsPromise {
        todo!()
    }
}

pub fn create_module(context: &mut Context) -> JsResult<(JsString, Module)> {
    Ok((js_string!("gui"), js::boa_module(None, context)))
}
