use colored::*;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use clap::Parser;
use std::error::Error;
use clap::{arg, command};
extern crate globwalk;

#[derive(PartialEq)]
enum FileOutcome {
    Renamed,
    RenameWasNoop,
    Unchanged,
}

struct FileToRename {
    full_path_before: PathBuf,
    full_path_after: PathBuf,
    filename_before: OsString,
    filename_after: OsString,
    outcome: FileOutcome,
}

#[derive(PartialEq)]
enum ActionWhenStuck {
    Retry,
    Skip,
    Abort,
    Rollback,
}

#[derive(PartialEq)]
enum ActionWhenStuckRollingBack {
    Retry,
    Skip,
    AbortRollback,
}

macro_rules! die
{
    ($($arg:expr),+) => {{
        print!("{}", "ERROR. ".red());
        println!($($arg), +);
        exit(1);
    }}
}

#[derive(Parser, Debug)] // requires `derive` feature
#[command(term_width = 0)] // Just to make testing across clap features easier
struct Arguments {
    /// Files selection pattern
    #[arg(short = 'f', long)]
    files: Vec<String>,

    /// Whether or not to include extensions in the renaming/patterns
    #[arg(short = 'x', long, default_value_t = true)]
    include_extensions: bool,

    /// Flag to dry-run the file renaming -- with this flag enabled the file-renaming map is simply printed to std-out
    #[arg(short = 'd', long, default_value_t = false)]
    dry_run: bool,
}

fn main() {
    let args = Arguments::parse();
    let mut files = list_files(&args);
    handle_degenerate_cases(&args, &files);

    let buffer_filename = std::env::temp_dir().join(".rename_buffer");
    write_filenames_to_buffer(&buffer_filename, &files);
    let _ = read_filenames_from_buffer(&buffer_filename, &mut files, &args);

    execute_rename(&args, &mut files);
    print_state(&files);
}

fn list_files(args: &Arguments) -> Vec<FileToRename> {
    let mut filenames = Vec::<FileToRename>::new();
    let mut invalid_indices = Vec::<usize>::new();
    let files = &args.files;

    for (index, file) in files.into_iter().enumerate() {
        let glob_result = globwalk::glob(&file);
        let paths = match glob_result {
            Ok(g) => g,
            Err(_) => {
                invalid_indices.push(index);
                continue;
            }
        };

        for path in paths {
            let path = match path {
                Ok(path) => path,
                Err(_) => {
                    invalid_indices.push(index);
                    continue;
                }
            };

            let relevant_part_of_file_name = if args.include_extensions {
                path.file_name()
            } else {
                path.file_stem()
            };

            let relevant_part_of_file_name = relevant_part_of_file_name
                .unwrap_or_else(|| die!("Unable to get file name out of path."));

            filenames.push(FileToRename {
                full_path_before: path,
                full_path_after: PathBuf::new(),
                filename_before: relevant_part_of_file_name.to_owned(),
                filename_after: OsString::new(),
                outcome: FileOutcome::Unchanged,
            });
        }
    }

    match invalid_indices.len() {
        0 => filenames,
        1 => die!(
            "Unable to create search pattern from argument #{}.",
            invalid_indices[0]
        ),
        _ => {
            let string_indices: Vec<String> =
                invalid_indices.iter().map(|n| format!("#{}", n)).collect();
            let (last, rest) = string_indices.split_last().unwrap();
            die!(
                "Unable to create search pattern from arguments {} and {}.",
                rest.join(", "),
                last
            )
        }
    }
}

fn handle_degenerate_cases(args: &Arguments, files: &Vec<FileToRename>) {
    if files.len() == 0 {
        if args.files.len() == 1 {
            println!("No files matched pattern.");
        } else {
            println!("No files matched any patterns.");
        }
        exit(0);
    }
}

fn write_filenames_to_buffer(buffer_filename: &Path, files: &Vec<FileToRename>) {
    let buffer_file = match File::create(&buffer_filename) {
        Ok(file) => file,
        Err(_) => die!("Unable to open buffer file for writing."),
    };
    let mut writer = LineWriter::new(buffer_file);

    for n in 0..files.len() {
        let file = &files[n];
        let filename_before = file
            .filename_before
            .to_str()
            .unwrap_or_else(|| die!("Unable to get string for filename."));
        // let newline = if n < files.len() - 1 { "\n" } else { "" };
        let newline = "\n";
        write!(&mut writer, "{}{}", filename_before, newline)
            .unwrap_or_else(|_| die!("Unable to write filenames to buffer file."));
    }
}

fn read_filenames_from_buffer(
    buffer_filename: &Path,
    files: &mut Vec<FileToRename>,
    args: &Arguments,
) -> Result<(), Box<dyn Error>> {
    let filenames_coming_in = read_filenames_from_file(buffer_filename)?;
    validate_filenames(files.len(), &filenames_coming_in)?;

    for (file, new_filename) in files.iter_mut().zip(filenames_coming_in.iter()) {
        file.filename_after = if args.include_extensions {
            new_filename.clone().into()
        } else {
            let extension = file.full_path_before.extension();
            new_filename
                .clone()
                .with_extension(extension.unwrap_or_default())
                .into()
        };
        file.full_path_after = file.full_path_before.with_file_name(&file.filename_after);
    }

    Ok(())
}

fn read_filenames_from_file(
    buffer_filename: &Path,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let content = std::fs::read_to_string(buffer_filename)?;
    let filenames_coming_in: Vec<PathBuf> = content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                Some(PathBuf::from(trimmed))
            } else {
                None
            }
        })
        .collect();
    Ok(filenames_coming_in)
}

fn validate_filenames(
    expected_count: usize,
    filenames_coming_in: &[PathBuf],
) -> Result<(), Box<dyn Error>> {
    if filenames_coming_in.len() < expected_count {
        return Err(format!(
            "Not enough filenames in text file after edit ({} instead of {}).",
            filenames_coming_in.len(),
            expected_count
        )
        .into());
    } else if filenames_coming_in.len() > expected_count {
        return Err(format!(
            "Too many filenames in text file after edit ({} instead of {}).",
            filenames_coming_in.len(),
            expected_count
        )
        .into());
    }
    Ok(())
}


fn execute_rename(args: &Arguments, files: &mut Vec<FileToRename>) {
    fn rename_file_if_safe(p: &Path, q: &Path) -> Result<(), ()> {
        if q.exists() {
            return Err(());
        };
        match fs::rename(p, q) {
            Ok(_) => Ok(()),
            Err(_) => Err(()),
        }
    }

    if args.dry_run == true {
        for file in files {
            println!(
                "{} -> {}",
                file.full_path_before.display(),
                file.full_path_after.display()
            );
        }
        exit(0);
    }

    let mut index = 0;
    let mut rollback = false;
    while index < files.len() {
        let file = &mut files[index];

        if file.full_path_after == file.full_path_before {
            file.outcome = FileOutcome::RenameWasNoop;
            index += 1;
            continue;
        }

        match rename_file_if_safe(&file.full_path_before, &file.full_path_after) {
            Ok(_) => {
                file.outcome = FileOutcome::Renamed;
                index += 1;
            }
            Err(_) => die!("file renaming was not safe"),
        }
    }

    if rollback == true {
        println!("Undoing renames...");

        index = 0;
        while index < files.len() {
            let file = &mut files[index];
            if file.outcome != FileOutcome::Renamed {
                index += 1;
                continue;
            }

            match rename_file_if_safe(&file.full_path_after, &file.full_path_before) {
                Ok(_) => {
                    file.outcome = FileOutcome::Unchanged;
                    index += 1;
                    continue;
                }
                Err(_) => die!("file renaming was not safe"),
            }
        }
    }
}

fn print_state(files: &Vec<FileToRename>) {
    let mut renamed = 0;
    let mut noop = 0;
    let mut unchanged = 0;

    for f in files {
        match f.outcome {
            FileOutcome::Renamed => renamed += 1,
            FileOutcome::RenameWasNoop => noop += 1,
            FileOutcome::Unchanged => unchanged += 1,
        }
    }

    if unchanged == 0 {
        println!("{}  renamed             ... {}", "DONE.".green(), renamed);
    } else {
        println!("{}  renamed             ... {}", "DONE.".yellow(), renamed);
    }
    if noop > 0 {
        println!("       skipped (no change) ... {}", noop);
    }
    if unchanged > 0 {
        println!("       skipped (problem)   ... {}", unchanged);
    }
}
