fn main() {
    println!("cargo:rerun-if-env-changed=SKK_RULES_PATH");
    let path =
        std::env::var("SKK_RULES_PATH").unwrap_or_else(|_| "/usr/share/libskk/rules".to_owned());
    println!("cargo:rustc-env=SKK_RULES_PATH={path}");
}
