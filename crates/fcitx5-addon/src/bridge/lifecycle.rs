use super::*;

/// Applies any completed asynchronous candidate evaluations.
///
/// # Safety
/// `backend` must be a live pointer returned by this library and callback must
/// be callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_poll_rerank(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: UpdateCallback,
) -> bool {
    // SAFETY: backend is created and exclusively owned by the C++ engine.
    let Some(backend) = (unsafe { backend.as_mut() }) else {
        return false;
    };
    if let Some(update) = backend.poll_rerank() {
        with_update(&update, backend.rerank_pending(), user_data, callback);
    }
    backend.rerank_pending()
}

/// Returns the inotify descriptor used for configuration changes.
///
/// # Safety
/// `backend` must be a live pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_config_watch_fd(backend: *mut Backend) -> i32 {
    // SAFETY: backend is exclusively owned by the C++ engine.
    unsafe { backend.as_ref() }.map_or(-1, Backend::config_watch_fd)
}

/// Applies a pending configuration-file change and reports its result.
///
/// # Safety
/// `backend` must be live and `callback` must be callable for this invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_reload_config_if_changed(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: MessageCallback,
) -> bool {
    // SAFETY: backend is exclusively owned by the C++ engine.
    let Some(backend) = (unsafe { backend.as_mut() }) else {
        return false;
    };
    let Some(result) = backend.reload_config_if_changed() else {
        return false;
    };
    match result {
        Ok(()) if backend.model_reload_pending() => {
            send_message(
                user_data,
                callback,
                true,
                "設定を再読み込みしました。AIモデルを更新中です",
            );
        }
        Ok(()) => send_message(user_data, callback, true, "設定を再読み込みしました"),
        Err(error) => send_message(
            user_data,
            callback,
            false,
            &format!("設定の再読み込みに失敗しました: {error:#}"),
        ),
    }
    true
}

/// Reports completion of an asynchronous model reload.
///
/// # Safety
/// `backend` must be live and `callback` must be callable for this invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_poll_model_reload(
    backend: *mut Backend,
    user_data: *mut c_void,
    callback: MessageCallback,
) -> bool {
    // SAFETY: backend is exclusively owned by the C++ engine.
    let Some(backend) = (unsafe { backend.as_mut() }) else {
        return false;
    };
    if let Some(Err(error)) = backend.poll_model_reload() {
        send_message(
            user_data,
            callback,
            false,
            &format!("AIモデルの更新に失敗しました。以前のモデルを使用します: {error:#}"),
        );
    }
    backend.model_reload_pending()
}

/// Creates the backend from a TOML file.
///
/// # Safety
/// `config_path` must point to a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_new(
    config_path: *const c_char,
    user_data: *mut c_void,
    callback: ModelDownloadCallback,
) -> *mut Backend {
    let config = if config_path.is_null() {
        BackendConfig::from_xdg()
    } else {
        let Some(config_path) = (unsafe { required_str(config_path) }) else {
            return ptr::null_mut();
        };
        BackendConfig::from_toml_file(Path::new(config_path))
    };
    let user_data = user_data as usize;
    let model_events = Arc::new(move |event| {
        let (kind, message) = match event {
            ModelDownloadEvent::Started { reference } => {
                (0, format!("AIモデルをダウンロードしています: {reference}"))
            }
            ModelDownloadEvent::Completed { reference } => (
                1,
                format!("AIモデルのダウンロードが完了しました: {reference}"),
            ),
            ModelDownloadEvent::Failed { reference, error } => (
                2,
                format!("AIモデルのダウンロードに失敗しました: {reference}: {error}"),
            ),
        };
        let message = nul_terminated(&message);
        // SAFETY: The C++ engine owns user_data until its initialization thread is joined.
        unsafe { callback(user_data as *mut c_void, kind, message.as_ptr().cast()) };
    });
    match config.and_then(|config| Backend::load_with_model_events(config, model_events)) {
        Ok(backend) => Box::into_raw(Box::new(backend)),
        Err(error) => {
            eprintln!("skk-zenz: backend initialization failed: {error:#}");
            ptr::null_mut()
        }
    }
}

/// Destroys a backend returned by `skk_zenz_backend_new`.
///
/// # Safety
/// `backend` must either be null or a live pointer returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skk_zenz_backend_free(backend: *mut Backend) {
    if !backend.is_null() {
        // SAFETY: Ownership is transferred back exactly once by the C++ engine.
        drop(unsafe { Box::from_raw(backend) });
    }
}
