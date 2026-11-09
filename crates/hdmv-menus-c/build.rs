//! The C header, made from the code on every build: crates/hdmv-menus-c/include/hdmv_menus.h.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    let config = cbindgen::Config::from_file(format!("{dir}/cbindgen.toml")).expect("cbindgen.toml");
    match cbindgen::generate_with_config(&dir, config) {
        Ok(bindings) => {
            bindings.write_to_file(format!("{dir}/include/hdmv_menus.h"));
        }
        // (a header that won't generate mustn't stop a build: the last one stays)
        Err(e) => println!("cargo:warning=the C header wasn't made: {e}"),
    }
}
