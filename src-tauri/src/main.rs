#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use learnloom_m0::app::{AppCore, AppError};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
#[tauri::command(rename_all = "snake_case")]
fn subscribe_updates(
    state: tauri::State<'_, Arc<AppCore>>,
    conversation_id: String,
    channel: tauri::ipc::Channel<learnloom_m0::chat::Update>,
) -> Result<Value, AppError> {
    let core = state.inner().clone();
    let (id, kb, mut receiver, token) = core
        .subscribe_chat(&conversation_id)
        .map_err(AppError::from)?;
    let result = json!({"subscription_id":id,"kb_id":kb});
    tauri::async_runtime::spawn(async move {
        loop {
            let next = tokio::select! {_=token.cancelled()=>break,next=receiver.recv()=>next};
            match next {
                Ok(event) if event.kb_id == kb && event.conversation_id == conversation_id => {
                    if channel.send(event).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let event = learnloom_m0::chat::Update {
                        kb_id: kb.clone(),
                        conversation_id: conversation_id.clone(),
                        entity_type: "message".into(),
                        entity_id: String::new(),
                        job_id: None,
                        run_id: None,
                        sequence: 0,
                        kind: "resync".into(),
                        payload: json!({}),
                    };
                    if channel.send(event).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        core.unsubscribe_chat(&id);
    });
    Ok(result)
}
#[tauri::command(rename_all = "snake_case")]
fn unsubscribe_updates(state: tauri::State<'_, Arc<AppCore>>, subscription_id: String) {
    state.unsubscribe_chat(&subscription_id);
}
#[tauri::command]
async fn learnloom(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppCore>>,
    operation: String,
    args: Value,
) -> Result<Value, AppError> {
    let core = state.inner().clone();
    let result: Result<Value, String> = match operation.as_str() {
        "choose_kb_directory" | "choose_backup_directory" => {
            let handle = app.clone();
            let chosen = tauri::async_runtime::spawn_blocking(move || {
                handle
                    .dialog()
                    .file()
                    .set_title("选择空目录或已有Learnloom知识库")
                    .blocking_pick_folder()
            })
            .await
            .map_err(|e| AppError::from(e.to_string()))?;
            match chosen {
                Some(path) => core.grant_directory(
                    &path
                        .into_path()
                        .map_err(|e| AppError::from(e.to_string()))?,
                ),
                None => Ok(json!({"cancelled":true})),
            }
        }
        "choose_source_file" => {
            let handle = app.clone();
            let chosen = tauri::async_runtime::spawn_blocking(move || {
                handle
                    .dialog()
                    .file()
                    .set_title("导入UTF-8文本或Markdown")
                    .add_filter("文本与Markdown", &["txt", "md", "markdown"])
                    .blocking_pick_file()
            })
            .await
            .map_err(|e| AppError::from(e.to_string()))?;
            match chosen {
                Some(path) => core.grant_file(
                    &path
                        .into_path()
                        .map_err(|e| AppError::from(e.to_string()))?,
                ),
                None => Ok(json!({"cancelled":true})),
            }
        }
        "open_external_url" => {
            let url = args["url"]
                .as_str()
                .ok_or_else(|| AppError::from("INVALID_URL".to_string()))?;
            let parsed =
                reqwest::Url::parse(url).map_err(|_| AppError::from("INVALID_URL".to_string()))?;
            if parsed.scheme() != "https"
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                Err("UNSAFE_URL".into())
            } else {
                app.opener()
                    .open_url(parsed.as_str(), None::<&str>)
                    .map_err(|_| "OPEN_URL_FAILED".into())
                    .map(|_| json!({"opened":true}))
            }
        }
        _ => core.dispatch(&operation, args).await,
    };
    if result.is_ok() && operation == "create_or_open_kb" {
        core.resume_queued().map_err(AppError::from)?;
    }
    result.map_err(AppError::from)
}
fn main() {
    tauri::Builder::default()
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                if let Some(core) = webview.try_state::<Arc<AppCore>>() {
                    // Reload drops JS callbacks. Stop bridges, but keep jobs and saved messages.
                    core.close_subscriptions();
                }
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let profile = if let Some(path) = std::env::var_os("LEARNLOOM_PROFILE") {
                let path = std::path::PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(std::io::Error::other("LEARNLOOM_PROFILE must be absolute").into());
                }
                path
            } else {
                app.path().app_local_data_dir()?
            };
            let core = AppCore::new(&profile).map_err(std::io::Error::other)?;
            let resume = core.clone();
            tauri::async_runtime::spawn(async move {
                let _ = resume.resume_queued();
            });
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            learnloom,
            subscribe_updates,
            unsubscribe_updates
        ])
        .run(tauri::generate_context!())
        .expect("Learnloom desktop runtime failed");
}
