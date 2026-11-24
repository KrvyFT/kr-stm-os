mod cli;
mod elf;
mod image;
mod pack;

use std::process::ExitCode;

use cli::USAGE;

fn main() -> ExitCode {
    let args = match cli::parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("pack: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match pack::write(&args.output, &args.elfs) {
        Ok(bytes) => {
            println!(
                "{}: {} application(s), {bytes} bytes",
                args.output.display(),
                args.elfs.len(),
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("pack: {error}");
            ExitCode::FAILURE
        }
    }
}
