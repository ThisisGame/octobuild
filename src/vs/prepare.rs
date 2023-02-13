use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::compiler::{
    Arg, CommandInfo, CompilationArgs, CompilationTask, InputKind, OutputKind, PCHArgs, PCHUsage,
    Scope,
};
use crate::utils::{expand_response_files, find_param, ParamValue};

pub fn create_tasks(
    command: CommandInfo,
    args: &[String],
    run_second_cpp: bool,
) -> crate::Result<Vec<CompilationTask>> {
    let expanded_args = expand_response_files(&command.current_dir, args)?;

    let parsed_args = parse_arguments(expanded_args.iter())?;
    // Source file name.
    let mut input_sources = Vec::<PathBuf>::new();
    for input in parsed_args.iter().filter_map(|arg| match arg {
        Arg::Input { kind, file, .. } if *kind == InputKind::Source => Some(PathBuf::from(file)),
        _ => None,
    }) {
        input_sources.push(command.absolutize(&input)?);
    }
    if input_sources.is_empty() {
        return Err(crate::Error::from(
            "Can't find source file path.".to_string(),
        ));
    }
    // Precompiled header file name.
    let precompiled_file = match find_param(&parsed_args, |arg: &Arg| -> Option<PathBuf> {
        match arg {
            Arg::Input { kind, file, .. } if *kind == InputKind::Precompiled => {
                Some(PathBuf::from(file))
            }
            _ => None,
        }
    }) {
        ParamValue::None => None,
        ParamValue::Single(v) => Some(v),
        ParamValue::Many(v) => {
            return Err(crate::Error::from(format!(
                "Found too many precompiled header files: {v:?}"
            )));
        }
    };
    // Precompiled header file name.
    let pch_param = find_param(&parsed_args, |arg: &Arg| -> Option<(bool, String)> {
        match arg {
            Arg::Input { kind, file, .. } if *kind == InputKind::Marker => {
                Some((true, file.clone()))
            }
            Arg::Output { kind, file, .. } if *kind == OutputKind::Marker => {
                Some((false, file.clone()))
            }
            _ => None,
        }
    });
    let pch_usage: PCHUsage = match &pch_param {
        ParamValue::None => crate::Result::<PCHUsage>::Ok(PCHUsage::None),
        ParamValue::Single((input, path)) => {
            let precompiled_path = match precompiled_file {
                Some(v) => v,
                None => PathBuf::from(path).with_extension("pch"),
            };
            let precompiled_path_abs = command.absolutize(&precompiled_path)?;
            let pch_marker = if path.is_empty() {
                None
            } else {
                Some(OsString::from(path))
            };
            if *input {
                Ok(PCHUsage::In(PCHArgs {
                    path: precompiled_path,
                    path_abs: precompiled_path_abs,
                    marker: pch_marker,
                }))
            } else {
                Ok(PCHUsage::Out(PCHArgs {
                    path: precompiled_path,
                    path_abs: precompiled_path_abs,
                    marker: pch_marker,
                }))
            }
        }
        ParamValue::Many(v) => {
            return Err(crate::Error::from(format!(
                "Found too many precompiled header markers: {:?}",
                v.iter().map(|item| item.1.clone()).collect::<PathBuf>()
            )));
        }
    }?;

    // Output object file name.
    let output_param = find_param(&parsed_args, |arg: &Arg| -> Option<PathBuf> {
        match arg {
            Arg::Output { kind, file, .. } if *kind == OutputKind::Object => {
                Some(PathBuf::from(file))
            }
            _ => None,
        }
    });
    let output_object: Option<PathBuf> = match output_param {
        ParamValue::None => None,
        ParamValue::Single(v) => Some(command.absolutize(&v)?),
        ParamValue::Many(v) => {
            return Err(crate::Error::from(format!(
                "Found too many output object files: {v:?}"
            )));
        }
    };
    // Language
    let language: Option<String> = match find_param(&parsed_args, |arg: &Arg| -> Option<String> {
        match arg {
            Arg::Param { flag, value, .. } if *flag == "T" => Some(value.clone()),
            _ => None,
        }
    }) {
        ParamValue::None => None,
        ParamValue::Single(v) => Some(v),
        ParamValue::Many(v) => {
            return Err(crate::Error::from(format!(
                "Found too many output object files: {v:?}"
            )));
        }
    };
    let shared = Arc::new(CompilationArgs {
        args: parsed_args,
        pch_usage,
        command,
        deps_file: None,
        run_second_cpp,
    });
    input_sources
        .into_iter()
        .map(|input_source| {
            let language = language
                .as_ref()
                .map_or_else(|| detect_language(&input_source), |lang| Some(lang.clone()))
                .ok_or_else(|| {
                    format!(
                        "Can't detect file language by extension: {}",
                        input_source.to_string_lossy()
                    )
                })?;
            Ok(CompilationTask {
                shared: shared.clone(),
                language,
                output_object: get_output_object(&input_source, &output_object)?,
                input_source,
            })
        })
        .collect()
}

fn detect_language(path: &Path) -> Option<String> {
    println!("{}", path.to_string_lossy());
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("cpp") || ext.eq_ignore_ascii_case("cc") {
        Some("P".to_string())
    } else if ext.eq_ignore_ascii_case("c") {
        Some("C".to_string())
    } else {
        None
    }
}

fn get_output_object(
    input_source: &Path,
    output_object: &Option<PathBuf>,
) -> crate::Result<PathBuf> {
    let result = output_object.as_ref().map_or_else(
        || {
            assert!(input_source.is_absolute());
            Ok(input_source.with_extension("obj"))
        },
        |path| {
            assert!(path.is_absolute());
            if path.is_dir() {
                input_source
                    .file_name()
                    .map(|name| path.join(name).with_extension("obj"))
                    .ok_or_else(|| {
                        crate::Error::Generic(format!(
                            "Input file path does not contain file name: {}",
                            input_source.to_string_lossy()
                        ))
                    })
            } else {
                Ok(path.clone())
            }
        },
    )?;
    Ok(result)
}

fn parse_arguments<S: AsRef<str>, I: Iterator<Item = S>>(mut iter: I) -> Result<Vec<Arg>, String> {
    let mut result: Vec<Arg> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    while let Some(parse_result) = parse_argument(&mut iter) {
        match parse_result {
            Ok(arg) => {
                result.push(arg);
            }
            Err(e) => {
                errors.push(e);
            }
        }
    }
    if !errors.is_empty() {
        return Err(format!("Found unknown command line arguments: {errors:?}"));
    }
    Ok(result)
}

#[allow(clippy::cognitive_complexity)]
fn parse_argument<S: AsRef<str>, I: Iterator<Item = S>>(
    iter: &mut I,
) -> Option<Result<Arg, String>> {
    iter.next().map(|arg| {
        if has_param_prefix(arg.as_ref()) {
            let flag = &arg.as_ref()[1..];
            match is_spaceable_param(flag) {
                Some((prefix, scope)) => {
                    if flag == prefix {
                        match iter.next() {
                            Some(value) => {
                                if has_param_prefix(value.as_ref()) {
                                    Err(arg.as_ref().to_string())
                                } else {
                                    Ok(Arg::param(scope, prefix, value.as_ref(), true))
                                }
                            }
                            _ => Err(arg.as_ref().to_string()),
                        }
                    } else {
                        Ok(Arg::param(scope, prefix, &flag[prefix.len()..], false))
                    }
                }
                None => match flag {
                    "c" | "nologo" => Ok(Arg::flag(Scope::Ignore, flag)),
                    "bigobj" => Ok(Arg::flag(Scope::Compiler, flag)),
                    "FC" | "d2vzeroupper" | "fastfail" => Ok(Arg::flag(Scope::Shared, flag)),
                    "X" => Ok(Arg::flag(Scope::Preprocessor, flag)),
                    s if s.starts_with('T') => Ok(Arg::param(Scope::Ignore, "T", &s[1..], false)),
                    s if s.starts_with('O') => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with('G') => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("RTC") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with('Z') => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("d2Zi+") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("std:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("MP") => Ok(Arg::flag(Scope::Compiler, flag)),
                    s if s.starts_with("fsanitize=") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("MD") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("MT") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("EH") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("fp:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("arch:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("errorReport:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("source-charset:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("execution-charset:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("favor:") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("Fo") => Ok(Arg::output(OutputKind::Object, "Fo", &s[2..])),
                    s if s.starts_with("Fp") => {
                        Ok(Arg::input(InputKind::Precompiled, "Fp", &s[2..]))
                    }
                    s if s.starts_with("Yc") => Ok(Arg::output(OutputKind::Marker, "Yc", &s[2..])),
                    s if s.starts_with("Yu") => Ok(Arg::input(InputKind::Marker, "Yu", &s[2..])),
                    s if s.starts_with("Yl") => Ok(Arg::flag(Scope::Shared, flag)),
                    s if s.starts_with("FI") => {
                        Ok(Arg::param(Scope::Preprocessor, "FI", &s[2..], false))
                    }
                    s if s.starts_with("analyze") => Ok(Arg::flag(Scope::Shared, flag)),
                    _ => Err(arg.as_ref().to_string()),
                },
            }
        } else {
            Ok(Arg::Input {
                kind: InputKind::Source,
                flag: String::new(),
                file: arg.as_ref().to_string(),
            })
        }
    })
}

fn is_spaceable_param(flag: &str) -> Option<(&str, Scope)> {
    for prefix in ["D"] {
        if flag.starts_with(prefix) {
            return Some((prefix, Scope::Shared));
        }
    }
    for prefix in ["I", "sourceDependencies"] {
        if flag.starts_with(prefix) {
            return Some((prefix, Scope::Preprocessor));
        }
    }
    for prefix in ["W", "wd", "we", "wo", "w"] {
        if flag.starts_with(prefix) {
            return Some((prefix, Scope::Compiler));
        }
    }
    None
}

fn has_param_prefix(arg: &str) -> bool {
    arg.starts_with('/') || arg.starts_with('-')
}

#[test]
fn test_parse_argument() {
    let args: Vec<String> =
        "/TP /c /Yusample.h /Fpsample.h.pch /Fosample.cpp.o /DTEST /D TEST2 /arch:AVX /fsanitize=address \
         sample.cpp"
            .split(' ')
            .map(|x| x.to_string())
            .collect();
    assert_eq!(
        parse_arguments(args.iter()).unwrap(),
        [
            Arg::param(Scope::Ignore, "T", "P", false),
            Arg::flag(Scope::Ignore, "c"),
            Arg::input(InputKind::Marker, "Yu", "sample.h"),
            Arg::input(InputKind::Precompiled, "Fp", "sample.h.pch"),
            Arg::output(OutputKind::Object, "Fo", "sample.cpp.o"),
            Arg::param(Scope::Shared, "D", "TEST", false),
            Arg::param(Scope::Shared, "D", "TEST2", true),
            Arg::flag(Scope::Shared, "arch:AVX"),
            Arg::flag(Scope::Shared, "fsanitize=address"),
            Arg::input(InputKind::Source, "", "sample.cpp")
        ]
    )
}
