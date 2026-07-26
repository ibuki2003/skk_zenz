use std::ffi::{CStr, c_char, c_void};
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use skk_zenz_backend::zenz::ModelDownloadEvent;
use skk_zenz_backend::{
    Backend, BackendConfig, InputEvent, InputUpdate, Key, KeyEvent, Modifiers, NamedKey,
};

mod bridge {
    use super::*;

    mod ffi;
    mod input;
    mod lifecycle;

    use ffi::*;
}
