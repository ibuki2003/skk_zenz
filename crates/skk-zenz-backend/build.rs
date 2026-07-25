fn main() {
    println!("cargo:rerun-if-env-changed=LIBSKK_DATA_PATH");
    let path =
        std::env::var("LIBSKK_DATA_PATH").unwrap_or_else(|_| "/usr/share/libskk/rules".to_owned());
    println!("cargo:rustc-env=LIBSKK_DATA_PATH={path}");
}
