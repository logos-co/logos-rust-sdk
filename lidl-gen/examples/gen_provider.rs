fn main() {
    let src = std::fs::read_to_string("/tmp/rp.lidl").unwrap();
    let m = logos_lidl_gen::parse(&src).unwrap();
    let code = logos_lidl_gen::generate_provider(&m, "0.1.0");
    std::fs::write("/tmp/rust_provider_check/src/generated.rs", code).unwrap();
    println!("written");
}
