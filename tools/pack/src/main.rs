mod elf;
mod image;

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use image::{App, MAX_APPS};

const USAGE: &str = "Usage: pack -o <IMAGE> [--] [ELF ...]\n\n\
    Pack ARM ELF files into a KAPP v1 image in the given boot order.\n\n\
    Options:\n  -o, --output <IMAGE>  Image output path\n  -h, --help            Print help";

struct Args {
    output: PathBuf,
    elfs: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let result = match parse_args() {
        Ok(Some(args)) => pack(args),
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("pack: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pack: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut output = None;
    let mut elfs = Vec::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_encoded_bytes() {
            b"--" => {
                elfs.extend(args.map(PathBuf::from));
                break;
            }
            b"-h" | b"--help" => return Ok(None),
            b"-o" | b"--output" => {
                let path = args
                    .next()
                    .filter(|arg| arg == "-" || !arg.as_encoded_bytes().starts_with(b"-"))
                    .ok_or("missing output path")?;
                output = Some(PathBuf::from(path));
            }
            bytes if bytes.starts_with(b"--output=") => {
                output = Some(attached_output(&bytes[b"--output=".len()..]));
            }
            bytes if bytes.starts_with(b"-o") => {
                let value = &bytes[b"-o".len()..];
                output = Some(attached_output(value.strip_prefix(b"=").unwrap_or(value)));
            }
            bytes if bytes.starts_with(b"-") && bytes != b"-" => {
                return Err(format!("unknown option {}", arg.to_string_lossy()));
            }
            _ => elfs.push(PathBuf::from(arg)),
        }
    }
    let output = output.ok_or("missing -o <IMAGE>")?;
    Ok(Some(Args { output, elfs }))
}

fn attached_output(encoded_path: &[u8]) -> PathBuf {
    // SAFETY: These bytes came from OsStr::as_encoded_bytes and were split
    // immediately after a known nonempty ASCII option prefix.
    PathBuf::from(unsafe { OsStr::from_encoded_bytes_unchecked(encoded_path) })
}

fn pack(args: Args) -> Result<(), String> {
    if args.elfs.len() > MAX_APPS {
        return Err(format!("at most {MAX_APPS} applications per image"));
    }

    let existing_output = fs::canonicalize(&args.output).ok();
    let mut names = HashSet::new();
    let mut apps = Vec::with_capacity(args.elfs.len());
    for path in &args.elfs {
        let input =
            fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if existing_output.as_ref() == Some(&input) {
            return Err("output must not overwrite an input ELF".into());
        }

        let name = app_name(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !names.insert(name) {
            return Err(format!("{}: duplicate application name", path.display()));
        }

        let bytes = fs::read(&input).map_err(|error| format!("{}: {error}", path.display()))?;
        let (entry, segments) =
            elf::parse(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        apps.push(App {
            name,
            entry,
            segments,
        });
    }

    let image = image::encode(&apps)?;
    write_image(&args.output, &image)
        .map_err(|error| format!("{}: {error}", args.output.display()))?;
    println!(
        "{}: {} application(s), {} bytes",
        args.output.display(),
        apps.len(),
        image.len(),
    );
    Ok(())
}

fn app_name(path: &Path) -> Result<[u8; 16], &'static str> {
    let name = if path.extension() == Some(OsStr::new("elf")) {
        path.file_stem()
    } else {
        path.file_name()
    };
    let name = name.map(OsStr::as_encoded_bytes).unwrap_or_default();
    if !(1..=16).contains(&name.len())
        || !name
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(c))
    {
        return Err("expected a 1–16 byte ASCII application name (letters, digits, _, . or -)");
    }

    let mut field = [0; 16];
    field[..name.len()].copy_from_slice(name);
    Ok(field)
}

fn write_image(output: &Path, image: &[u8]) -> io::Result<()> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    // Exclusive creation allows concurrent packers to write separate temporary files.
    let mut index = 0;
    let (temp_path, mut temp) = loop {
        let path = parent.join(format!(".pack-apps-{}-{index}", std::process::id()));
        if path.file_name() == output.file_name() {
            index += 1;
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => index += 1,
            Err(error) => return Err(error),
        }
    };

    let written = temp.write_all(image);
    drop(temp);
    // Renaming within the same directory publishes the complete image in one step.
    let result = written.and_then(|()| fs::rename(&temp_path, output));
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}
