# Task W0-T2: Fix Collapsible Match Clippy Warnings

## Agent
Clippy Collapsible Match Fixer

## Task
Fix 3 "collapsible if" clippy warnings in x86_64 disasm.rs

## File Modified
- `/home/z/my-project/vuma/src/codegen/src/x86_64/disasm.rs`

## Changes
Replaced nested if-else blocks inside match arms in `decode_modrm_mem()` with pattern guards:

### Before (3 warnings):
```rust
let disp = match mod_bits {
    0 => {
        if rm_raw == 5 {
            if adv + 4 <= bytes.len() {
                let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
                adv += 4;
                d
            } else {
                0
            }
        } else {
            0
        }
    }
    1 => {
        if adv < bytes.len() {
            let d = bytes[adv] as i8 as i32;
            adv += 1;
            d
        } else {
            0
        }
    }
    2 => {
        if adv + 4 <= bytes.len() {
            let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
            adv += 4;
            d
        } else {
            0
        }
    }
    _ => 0,
};
```

### After (0 warnings):
```rust
let disp = match mod_bits {
    0 if rm_raw == 5 && adv + 4 <= bytes.len() => {
        let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
        adv += 4;
        d
    }
    1 if adv < bytes.len() => {
        let d = bytes[adv] as i8 as i32;
        adv += 1;
        d
    }
    2 if adv + 4 <= bytes.len() => {
        let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
        adv += 4;
        d
    }
    0 | 1 | 2 | _ => 0,
};
```

## Verification
- `cargo clippy -p vuma-codegen 2>&1 | grep "collapsible"` — ZERO warnings
- `cargo test -p vuma-codegen -- -q` — 675 passed, 0 failed
