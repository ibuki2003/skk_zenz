use super::*;

#[repr(C)]
pub struct FfiCandidate {
    text: *const c_char,
    annotation: *const c_char,
}

#[repr(C)]
pub struct FfiUpdate {
    consumed: bool,
    preedit: *const c_char,
    commit: *const c_char,
    candidates: *const FfiCandidate,
    candidate_count: usize,
    selected_candidate: usize,
    input_mode: u8,
    rerank_pending: bool,
    evaluated_candidate_count: usize,
    total_candidate_count: usize,
    candidate_window_visible: bool,
    candidate_page_start: usize,
    candidate_page_size: usize,
    show_annotations: bool,
}

pub type UpdateCallback = unsafe extern "C" fn(*mut c_void, *const FfiUpdate);
pub type MessageCallback = unsafe extern "C" fn(*mut c_void, bool, *const c_char);
pub type ModelDownloadCallback = unsafe extern "C" fn(*mut c_void, u8, *const c_char);

pub(super) unsafe fn required_str<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    // SAFETY: The C++ adapter keeps the NUL-terminated string alive for this call.
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}

pub(super) fn with_update(
    update: &InputUpdate,
    rerank_pending: bool,
    user_data: *mut c_void,
    callback: UpdateCallback,
) {
    let preedit = nul_terminated(&update.preedit);
    let commit = update.commit.as_deref().map(nul_terminated);
    let texts: Vec<Vec<u8>> = update
        .candidates
        .iter()
        .map(|candidate| nul_terminated(&candidate.output))
        .collect();
    let annotations: Vec<Option<Vec<u8>>> = update
        .candidates
        .iter()
        .map(|candidate| candidate.annotation.as_deref().map(nul_terminated))
        .collect();
    let candidates: Vec<FfiCandidate> = texts
        .iter()
        .zip(&annotations)
        .map(|(text, annotation)| FfiCandidate {
            text: text.as_ptr().cast(),
            annotation: annotation
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr().cast()),
        })
        .collect();
    let ffi_update = FfiUpdate {
        consumed: update.consumed,
        preedit: preedit.as_ptr().cast(),
        commit: commit
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr().cast()),
        candidates: candidates.as_ptr(),
        candidate_count: candidates.len(),
        selected_candidate: update.selected_candidate.unwrap_or(usize::MAX),
        input_mode: match update.input_mode {
            skk_zenz_backend::InputMode::Hiragana => 0,
            skk_zenz_backend::InputMode::Katakana => 1,
            skk_zenz_backend::InputMode::HankakuKatakana => 2,
            skk_zenz_backend::InputMode::Latin => 3,
            skk_zenz_backend::InputMode::WideLatin => 4,
        },
        rerank_pending,
        evaluated_candidate_count: update.rerank_progress.map_or(0, |progress| progress.0),
        total_candidate_count: update.rerank_progress.map_or(0, |progress| progress.1),
        candidate_window_visible: update.candidate_window_visible,
        candidate_page_start: update.candidate_page_start,
        candidate_page_size: update.candidate_page_size,
        show_annotations: update.show_annotations,
    };
    // SAFETY: All pointers in ffi_update remain valid for the synchronous callback.
    unsafe { callback(user_data, &raw const ffi_update) };
}

pub(super) fn nul_terminated(value: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = value
        .as_bytes()
        .iter()
        .copied()
        .filter(|byte| *byte != 0)
        .collect();
    bytes.push(0);
    bytes
}

pub(super) fn send_message(
    user_data: *mut c_void,
    callback: MessageCallback,
    success: bool,
    message: &str,
) {
    let message = nul_terminated(message);
    // SAFETY: message remains alive for the synchronous callback.
    unsafe { callback(user_data, success, message.as_ptr().cast()) };
}
