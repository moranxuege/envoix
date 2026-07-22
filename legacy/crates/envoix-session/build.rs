use std::env;
use std::path::Path;

const NAT_TEST_CA_PATH: &str = "ENVOIX_NAT_TEST_CA_DER_PATH";

fn main() {
    println!("cargo:rustc-check-cfg=cfg(envoix_nat_test_local_ca)");
    println!("cargo:rerun-if-env-changed={NAT_TEST_CA_PATH}");

    if let Some(path) = env::var_os(NAT_TEST_CA_PATH) {
        println!("cargo:rustc-cfg=envoix_nat_test_local_ca");
        println!("cargo:rerun-if-changed={}", Path::new(&path).display());
    }
}
