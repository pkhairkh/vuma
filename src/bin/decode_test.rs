use vuma_codegen::arm64::Instruction;

fn main() {
    let words: Vec<(u32, &str)> = vec![
        (0xA9BF7BFD, "STP X29, X30, [SP, #-48]! (print_hex prologue)"),
        (0x910003FD, "ADD X29, SP, #0"),
        (0xA90127E9, "STP X9, X10, [SP, #16] (claimed)"),
        (0xA9020441, "STP X1, X2, [SP, #32] (claimed)"),
        (0xA9052C63, "STP X3, X8, [SP, #40] (claimed)"),
        (0xA912BE09, "STP X9, X10, [SP, #16] (correct)"),
        (0xA90A23E3, "STP X3, X8, [SP, #40] (correct)"),
        (0xA94127E9, "LDP X9, X10, [SP, #16] (claimed restore)"),
        (0xA9420441, "LDP X1, X2, [SP, #32] (claimed restore)"),
        (0xA9452C63, "LDP X3, X8, [SP, #40] (claimed restore)"),
        (0xA8C37BFD, "LDP X29, X30, [SP], #48 (post-index)"),
        (0xA9C27BFD, "STP X29, X30, [SP, #-64]! (print_int prologue)"),
        (0xA9030463, "STP X3, X8, [SP, #48] (claimed)"),
        (0xA9430463, "LDP X3, X8, [SP, #48] (claimed)"),
        (0xA8C27BFD, "LDP X29, X30, [SP], #64 (post-index)"),
        (0xA9BE7BFD, "STP X29, X30, [SP, #-32]! (newline prologue)"),
        (0xA8C27BFD, "LDP X29, X30, [SP], #32 (post-index)"),
    ];
    for (w, comment) in words {
        let decoded = Instruction::decode(w).map(|i| format!("{}", i)).unwrap_or_else(|| "???".to_string());
        println!("0x{:08X}  {}  ; {}", w, decoded, comment);
    }
}
