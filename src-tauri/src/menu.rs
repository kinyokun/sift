//! The menu bar on macOS. The one Tauri makes on its own comes in English
//! only; this is the same menu in the language the app is shown in. The
//! window says which that is once it knows (`set_menu_language`), and it is
//! remembered for the next launch, so the bar is right from the start.

use tauri::AppHandle;

#[cfg(target_os = "macos")]
mod bar {
    use crate::state::AppState;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};
    use tauri::{AppHandle, Manager};

    /// The language of the menu that is up.
    static SHOWN: Mutex<Option<String>> = Mutex::new(None);

    fn remembered(app: &AppHandle) -> PathBuf {
        app.state::<AppState>().data_dir.join("menu-language")
    }

    /// The words are the ones macOS itself uses in each language.
    fn build(app: &AppHandle, chinese: bool) -> tauri::Result<Menu<tauri::Wry>> {
        let say = |english: &'static str, chinese_words: &'static str| if chinese { chinese_words } else { english };
        let name = app.package_info().name.clone();
        let named = |english: &str, chinese_words: &str| format!("{} {name}", if chinese { chinese_words } else { english });
        let about = AboutMetadata {
            name: Some(name.clone()),
            version: Some(app.package_info().version.to_string()),
            copyright: app.config().bundle.copyright.clone(),
            ..Default::default()
        };
        let separator = || PredefinedMenuItem::separator(app);
        Menu::with_items(
            app,
            &[
                &Submenu::with_items(
                    app,
                    &name,
                    true,
                    &[
                        &PredefinedMenuItem::about(app, Some(&named("About", "关于")), Some(about))?,
                        &separator()?,
                        &PredefinedMenuItem::services(app, Some(say("Services", "服务")))?,
                        &separator()?,
                        &PredefinedMenuItem::hide(app, Some(&named("Hide", "隐藏")))?,
                        &PredefinedMenuItem::hide_others(app, Some(say("Hide Others", "隐藏其他")))?,
                        &PredefinedMenuItem::show_all(app, Some(say("Show All", "全部显示")))?,
                        &separator()?,
                        &PredefinedMenuItem::quit(app, Some(&named("Quit", "退出")))?,
                    ],
                )?,
                &Submenu::with_items(app, say("File", "文件"), true, &[&PredefinedMenuItem::close_window(app, Some(say("Close Window", "关闭窗口")))?])?,
                &Submenu::with_items(
                    app,
                    say("Edit", "编辑"),
                    true,
                    &[
                        &PredefinedMenuItem::undo(app, Some(say("Undo", "撤销")))?,
                        &PredefinedMenuItem::redo(app, Some(say("Redo", "重做")))?,
                        &separator()?,
                        &PredefinedMenuItem::cut(app, Some(say("Cut", "剪切")))?,
                        &PredefinedMenuItem::copy(app, Some(say("Copy", "拷贝")))?,
                        &PredefinedMenuItem::paste(app, Some(say("Paste", "粘贴")))?,
                        &PredefinedMenuItem::select_all(app, Some(say("Select All", "全选")))?,
                    ],
                )?,
                &Submenu::with_items(app, say("View", "显示"), true, &[&PredefinedMenuItem::fullscreen(app, Some(say("Enter Full Screen", "进入全屏幕")))?])?,
                // By these names macOS knows the two menus it adds to itself.
                &Submenu::with_id_and_items(
                    app,
                    WINDOW_SUBMENU_ID,
                    say("Window", "窗口"),
                    true,
                    &[
                        &PredefinedMenuItem::minimize(app, Some(say("Minimize", "最小化")))?,
                        &PredefinedMenuItem::maximize(app, Some(say("Zoom", "缩放")))?,
                    ],
                )?,
                &Submenu::with_id_and_items(app, HELP_SUBMENU_ID, say("Help", "帮助"), true, &[])?,
            ],
        )
    }

    pub fn show(app: &AppHandle, language: &str) {
        let mut shown = SHOWN.lock().unwrap();
        if shown.as_deref() == Some(language) {
            return;
        }
        if build(app, language == "zh").and_then(|menu| app.set_menu(menu)).is_ok() {
            *shown = Some(language.to_string());
            let _ = std::fs::write(remembered(app), language);
        }
    }

    pub fn restore(app: &AppHandle) {
        if let Ok(language) = std::fs::read_to_string(remembered(app)) {
            show(app, language.trim());
        }
    }
}

/// Puts up the menu bar of the last launch; a first launch keeps Tauri's until the window has spoken.
pub fn restore(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    bar::restore(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// `lang` is the language the window shows: `zh` or `en`.
#[tauri::command]
pub fn set_menu_language(app: AppHandle, lang: String) {
    #[cfg(target_os = "macos")]
    bar::show(&app, &lang);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, lang);
}
