use std::{env::var, path::PathBuf};

fn generate_lib_binding(lib_name: &str, allowlist_items: &[&str], rustified_enums: &[&str]) {
    let lib = pkg_config::probe_library(lib_name).unwrap();

    let flags = lib
        .include_paths
        .iter()
        .map(|t| format!("-I{}", t.to_str().expect("utf-8?")))
        .collect::<Vec<String>>();

    let mut builder = bindgen::Builder::default()
        .header(format!("wrapper/{}.h", lib_name))
        .clang_args(flags.clone()) //? can't __.* or we kill bindgen internal symbols
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_debug(true)
        .impl_debug(true);

    for item in allowlist_items {
        builder = builder.allowlist_item(item);
    }

    for item in rustified_enums {
        builder = builder.rustified_enum(item);
    }

    let bindings = builder.generate().expect("fail to generate bindings");

    let out_path = PathBuf::from(var("OUT_DIR").unwrap()).join(format!("ffi_{}.rs", lib_name));
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}

fn main() {
    #[cfg(feature = "rga")]
    generate_lib_binding(
        "librga",
        &[
            "im.*",
            "importbuffer.*",
            "wrapbuffer.*",
            "releasebuffer_handle",
            ".*Rga.*",
            "IM.*",
            "get_bpp_from_format",
        ],
        &["IM_ERROR_INDEX", "_Rga_SURF_FORMAT", "IM_STATUS"],
    );

    #[cfg(feature = "rkcrypto")]
    generate_lib_binding("rkcrypto", &["rk.*", "RK.*"], &[]);
}
