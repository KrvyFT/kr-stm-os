use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    elf,
    image::{self, App, MAX_APPS},
};

pub(super) fn write(output: &Path, elfs: &[PathBuf]) -> Result<usize, String> {
    if elfs.len() > MAX_APPS {
        return Err(format!("at most {MAX_APPS} applications per image"));
    }

    let existing_output = fs::canonicalize(output).ok();
    let mut names = HashSet::new();
    let mut apps = Vec::with_capacity(elfs.len());
    for path in elfs {
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
    write_image(output, &image).map_err(|error| format!("{}: {error}", output.display()))?;
    Ok(image.len())
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
