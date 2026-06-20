use vuma_parser::{Parser, AstToScg};
use vuma_scg::{EdgeKind, NodePayload};
use vuma::pipeline::{CompileConfig, run_scg_transforms, OptLevel};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = if args.len() > 1 { &args[1] } else { "/tmp/test_alloc.vuma" };
    let source = std::fs::read_to_string(path).unwrap();
    let mut parser = Parser::new(&source);
    let result = parser.parse_program();
    if result.has_errors() { eprintln!("Parse errors"); return; }
    let ast = result.unwrap();
    let mut scg = { let mut c = AstToScg::new(); c.convert(&ast).unwrap() };
    let config = CompileConfig { opt_level: OptLevel::O0, ..Default::default() };
    let _ = run_scg_transforms(&mut scg, &config);
    eprintln!("=== SCG Nodes ===");
    for node in scg.nodes() {
        eprintln!("  Node {}: {:?}", node.id.as_u64(), node.payload);
    }
    eprintln!("\n=== SCG Edges ===");
    for edge in scg.edges() {
        eprintln!("  {} -{:?}-> {}", edge.source.as_u64(), edge.kind, edge.target.as_u64());
    }
}
