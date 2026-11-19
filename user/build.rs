fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rerun-if-changed=link.x");
    println!("cargo:rustc-link-arg-bins=-T{manifest_dir}/link.x");
}
