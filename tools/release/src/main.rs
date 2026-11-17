// SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
// SPDX-License-Identifier: FSL-1.1-ALv2

use ctypst_release::{Env, Fail, main_with};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{}", Fail::from(error));
            return ExitCode::FAILURE;
        }
    };
    match main_with(&args, &root, &Env::from_process()) {
        Ok(output) => {
            if let Some(line) = output {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        // Avoid printing command arguments: JSR's supported token argument is secret, and child
        // diagnostics can contain authentication context.
        Err(Fail::Usage(text)) => {
            eprintln!("{text}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
