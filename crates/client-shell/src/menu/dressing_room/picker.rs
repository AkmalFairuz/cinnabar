//! Platform skin and cape chooser, called only by the skin worker.

use std::path::PathBuf;
#[cfg(not(windows))]
use std::process::Command;

pub(super) fn pick(cape: bool) -> Result<Option<PathBuf>, String> {
    let title = if cape {
        "Import cape PNG"
    } else {
        "Import skin PNG, geometry JSON, or skin pack"
    };
    #[cfg(windows)]
    return launcher_host::desktop::windows::pick_file(
        title,
        if cape {
            "PNG images\0*.png\0\0"
        } else {
            "Skins\0*.png;*.json;*.mcpack\0\0"
        },
    )
    .map_err(|error| format!("The PNG chooser could not open ({error:#x})."));
    #[cfg(target_os = "linux")]
    if let Ok(path) = launcher_host::desktop::dbus::pick_file(
        title,
        "Skins",
        &if cape {
            vec!["*.png".to_owned()]
        } else {
            vec![
                "*.png".to_owned(),
                "*.json".to_owned(),
                "*.mcpack".to_owned(),
            ]
        },
    ) {
        return Ok(path);
    }
    #[cfg(not(windows))]
    {
        let commands = if cfg!(target_os = "macos") {
            let mut command = Command::new("osascript");
            let types = if cape {
                "public.png"
            } else {
                "public.png\", \"public.json\", \"public.zip-archive\", \"mcpack"
            };
            let script = format!(
                "POSIX path of (choose file with prompt \"{title}\" of type {{\"{types}\"}})"
            );
            command.args(["-e", &script]);
            vec![command]
        } else {
            let mut zenity = Command::new("zenity");
            zenity.args([
                "--file-selection",
                &format!("--title={title}"),
                if cape {
                    "--file-filter=PNG images | *.png"
                } else {
                    "--file-filter=Skins | *.png *.json *.mcpack"
                },
            ]);
            let mut kdialog = Command::new("kdialog");
            kdialog.args([
                "--getopenfilename",
                ".",
                if cape {
                    "*.png|PNG images"
                } else {
                    "*.png *.json *.mcpack|Skins"
                },
                "--title",
                title,
            ]);
            vec![zenity, kdialog]
        };
        for mut command in commands {
            match command.output() {
                Ok(output) if output.status.success() => {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
                }
                Ok(_) => return Ok(None),
                Err(_) => {}
            }
        }
        Err("The PNG chooser is unavailable.".to_owned())
    }
}

/// Asks the page to pick skin or cape files (`globalThis.cinnabarPickFiles` in `web/site/packs.js`)
/// and queues what the player chose, possibly nothing, as one [`super::Job::Picked`].
#[cfg(target_arch = "wasm32")]
pub(super) fn pick_in_page(cape: bool, commands: crossbeam_channel::Sender<super::Job>) {
    use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue};
    async fn chosen(cape: bool) -> Option<Vec<(String, Vec<u8>)>> {
        let pick: Function = Reflect::get(&js_sys::global(), &"cinnabarPickFiles".into())
            .ok()?
            .dyn_into()
            .ok()?;
        let extensions: Array = if cape {
            ["png"]
                .iter()
                .map(|extension| JsValue::from_str(extension))
                .collect()
        } else {
            ["png", "json", "mcpack"]
                .iter()
                .map(|extension| JsValue::from_str(extension))
                .collect()
        };
        let promise: Promise = pick
            .call1(&JsValue::NULL, &extensions)
            .ok()?
            .dyn_into()
            .ok()?;
        let picked: Array = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .ok()?
            .dyn_into()
            .ok()?;
        Some(
            picked
                .iter()
                .filter_map(|entry| {
                    let entry: Array = entry.dyn_into().ok()?;
                    let name = entry.get(0).as_string()?;
                    let bytes: Uint8Array = entry.get(1).dyn_into().ok()?;
                    Some((name, bytes.to_vec()))
                })
                .collect(),
        )
    }
    wasm_bindgen_futures::spawn_local(async move {
        let files = chosen(cape).await.unwrap_or_default();
        let _ = commands.send(super::Job::Picked { cape, files });
    });
}
