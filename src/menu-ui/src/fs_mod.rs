//! `1fpga:fs` — filesystem access for the frontend.
//!
//! Implements the promise-returning subset of the established
//! `1fpga:fs` typing that the frontend needs today (content scanning
//! and game-directory browsing), plus [`read_dir_entries`] which
//! returns `{name, dir, size}` objects so a directory scan doesn't
//! need a follow-up `isDir` syscall per entry.
//!
//! IO here is performed synchronously on the UI thread and returned
//! as an already-settled promise. That keeps the JS contract (always
//! a promise, errors arrive via rejection) while avoiding a worker
//! until one is warranted: the workloads are boot-time core scans
//! (a handful of ~50-entry directories) and on-demand game-directory
//! listings. If a FAT directory with thousands of entries ever shows
//! up as a frame hitch in the timing log, this moves onto a worker
//! thread with the same bridge mechanics as `crate::db` — the JS API
//! is already shaped for it.

use std::fs;
use std::path::Path;

use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::{Context, JsError, JsResult, JsValue, js_string};
use boa_macros::boa_module;

/// Wrap a fallible native result into a settled promise: `Ok` values
/// resolve, `Err` messages reject (so `try { await … } catch` works
/// the same as with the db module's worker-routed promises).
fn settle(result: Result<JsValue, String>, context: &mut Context) -> JsResult<JsPromise> {
    match result {
        Ok(v) => JsPromise::resolve(v, context),
        Err(msg) => {
            let err = JsError::from_opaque(js_string!(msg).into());
            JsPromise::reject(err, context)
        }
    }
}

fn list_dir(path: &str) -> Result<Vec<(String, bool, u64)>, String> {
    let rd = fs::read_dir(path).map_err(|e| format!("readDir {path}: {e}"))?;
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry.map_err(|e| format!("readDir {path}: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // File type + size from one metadata call; entries we cannot
        // stat (dangling symlinks on exFAT, permission oddities) are
        // skipped rather than failing the whole listing.
        let Ok(meta) = entry.metadata() else { continue };
        out.push((name, meta.is_dir(), meta.len()));
    }
    Ok(out)
}

#[boa_module]
#[boa(rename_all = "camelCase")]
mod js_module {
    use super::*;

    fn read_dir(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let result = match super::list_dir(&path) {
            Ok(entries) => {
                let arr = JsArray::new(context)?;
                for (name, _, _) in entries {
                    arr.push(js_string!(name), context)?;
                }
                Ok(arr.into())
            }
            Err(msg) => Err(msg),
        };
        super::settle(result, context)
    }

    fn read_dir_entries(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let result = match super::list_dir(&path) {
            Ok(entries) => {
                let arr = JsArray::new(context)?;
                for (name, dir, size) in entries {
                    let obj = boa_engine::object::ObjectInitializer::new(context)
                        .property(js_string!("name"), js_string!(name), Default::default())
                        .property(js_string!("dir"), dir, Default::default())
                        .property(js_string!("size"), size as f64, Default::default())
                        .build();
                    arr.push(obj, context)?;
                }
                Ok(arr.into())
            }
            Err(msg) => Err(msg),
        };
        super::settle(result, context)
    }

    fn is_dir(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let v = Path::new(&path).is_dir();
        JsPromise::resolve(JsValue::from(v), context)
    }

    fn is_file(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let v = Path::new(&path).is_file();
        JsPromise::resolve(JsValue::from(v), context)
    }

    fn file_size(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let result = fs::metadata(&path)
            .map(|m| JsValue::from(m.len() as f64))
            .map_err(|e| format!("fileSize {path}: {e}"));
        super::settle(result, context)
    }

    fn read_text_file(path: String, context: &mut Context) -> JsResult<JsPromise> {
        let result = fs::read_to_string(&path)
            .map(|s| js_string!(s).into())
            .map_err(|e| format!("readTextFile {path}: {e}"));
        super::settle(result, context)
    }

    fn mkdir(path: String, all: Option<bool>, context: &mut Context) -> JsResult<JsPromise> {
        let result = if all.unwrap_or(false) {
            fs::create_dir_all(&path)
        } else {
            fs::create_dir(&path)
        };
        super::settle(
            result
                .map(|_| JsValue::undefined())
                .map_err(|e| format!("mkdir {path}: {e}")),
            context,
        )
    }
}

/// Register the `1fpga:fs` module.
pub fn register(
    loader: &boa_engine::module::MapModuleLoader,
    context: &mut Context,
) -> JsResult<()> {
    loader.insert("1fpga:fs", js_module::boa_module(None, context));
    Ok(())
}
