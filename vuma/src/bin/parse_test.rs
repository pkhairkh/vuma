use vuma_parser::Parser;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = if args.len() > 1 { &args[1] } else { "examples/crc32.vuma" };
    let source = std::fs::read_to_string(path).unwrap();
    let mut parser = Parser::new(&source);
    let result = parser.parse_program();
    eprintln!("Parse errors: {}", result.errors.len());
    for err in &result.errors { eprintln!("  ERROR: {}", err); }
}
