// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

fn main() {
    let protoc_path = protoc_bin_vendored::protoc_bin_path().unwrap();

    unsafe {
        #[allow(clippy::disallowed_methods)]
        std::env::set_var("PROTOC", protoc_path);
    }

    tonic_build::configure()
        .out_dir("src/api/gen")
        .server_mod_attribute("controller.proto.v1", "#[cfg(feature = \"native\")]")
        .client_mod_attribute("controller.proto.v1", "#[cfg(feature = \"native\")]")
        .compile_protos(&["proto/v1/controller.proto"], &["proto/v1"])
        .unwrap();
}
