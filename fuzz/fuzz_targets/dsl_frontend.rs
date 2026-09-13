#![no_main]

use carolina_core::limits::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        let limits = Limits::tiny();
        if let Ok(module) = carolina_lang::parser::parse_module(source, &limits) {
            let _ = carolina_lang::lower::lower_module(&module, None);
        }
    }
});
