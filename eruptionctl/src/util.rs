/*
    This file is part of Eruption.

    Eruption is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    Eruption is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with Eruption.  If not, see <http://www.gnu.org/licenses/>.

    Copyright (c) 2019-2022, The Eruption Development Team
*/

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::constants;
use crate::manifest::{self, Manifest, ManifestError};
use crate::profiles;

type Result<T> = std::result::Result<T, eyre::Error>;

#[derive(Debug, thiserror::Error)]
pub enum UtilError {
    #[error("File not found: {description}")]
    FileNotFound { description: String },
}

pub fn get_profile_dirs() -> Vec<PathBuf> {
    let mut result = vec![];

    let config = crate::CONFIG.lock();

    let profile_dirs = config
        .as_ref()
        .unwrap()
        .get::<Vec<String>>("global.profile_dirs")
        .unwrap_or_else(|_| vec![]);

    let mut profile_dirs = profile_dirs
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();

    result.append(&mut profile_dirs);

    // if we could not determine a valid set of paths, use a hard coded fallback instead
    if result.is_empty() {
        log::warn!("Using default fallback profile directory");

        let path = PathBuf::from(constants::DEFAULT_PROFILE_DIR);
        result.push(path);
    }

    result
}

pub fn get_script_dirs() -> Vec<PathBuf> {
    let mut result = vec![];

    let config = crate::CONFIG.lock();

    let script_dirs = config
        .as_ref()
        .unwrap()
        .get::<Vec<String>>("global.script_dirs")
        .unwrap_or_else(|_| vec![]);

    let mut script_dirs = script_dirs
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<PathBuf>>();

    result.append(&mut script_dirs);

    // if we could not determine a valid set of paths, use a hard coded fallback instead
    if result.is_empty() {
        log::warn!("Using default fallback script directory");

        let path = PathBuf::from(constants::DEFAULT_SCRIPT_DIR);
        result.push(path);
    }

    result
}

pub fn enumerate_scripts() -> Result<Vec<Manifest>> {
    manifest::get_scripts()
}

/// Returns the absolute path of a script file
pub fn match_script_file(script: &Path) -> Result<PathBuf> {
    let scripts = manifest::get_script_files()?;

    for f in scripts {
        if f.file_name().unwrap_or_default() == script {
            return Ok(f);
        }
    }

    Err(ManifestError::ScriptEnumerationError {}.into())
}

pub fn enumerate_profiles() -> Result<Vec<profiles::Profile>> {
    let mut result = profiles::get_profiles()?;

    // sort profiles by their name
    result.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

    Ok(result)
}

/// Returns the associated manifest path in `PathBuf` for the script `script_path`.
pub fn get_manifest_for(script_file: &Path) -> PathBuf {
    let mut manifest_path = script_file.to_path_buf();
    manifest_path.set_extension("lua.manifest");

    manifest_path
}

pub fn is_file_accessible<P: AsRef<Path>>(p: P) -> std::io::Result<String> {
    fs::read_to_string(p)
}

pub fn edit_file<P: AsRef<Path>>(file_name: P) -> Result<()> {
    println!("Editing: {}", &file_name.as_ref().to_string_lossy());

    Command::new(std::env::var("EDITOR").unwrap_or_else(|_| "/usr/bin/nano".to_string()))
        .args(&[file_name.as_ref().to_string_lossy().to_string()])
        .status()?;

    Ok(())
}

pub fn match_profile_path<P: AsRef<Path>>(profile_file: &P) -> Result<PathBuf> {
    let profile_file = profile_file.as_ref();

    let mut result = Err(UtilError::FileNotFound {
        description: format!(
            "Could not find file in search path(s): {}",
            &profile_file.display()
        ),
    }
    .into());

    'DIR_LOOP: for dir in get_profile_dirs().iter() {
        let profile_path = dir.join(&profile_file);

        if let Ok(metadata) = fs::metadata(&profile_path) {
            if metadata.is_file() {
                result = Ok(profile_path);
                break 'DIR_LOOP;
            }
        }
    }

    result
}

pub fn match_script_path<P: AsRef<Path>>(script_file: &P) -> Result<PathBuf> {
    let script_file = script_file.as_ref();

    let mut result = Err(UtilError::FileNotFound {
        description: format!(
            "Could not find file in search path(s): {}",
            &script_file.display()
        ),
    }
    .into());

    'DIR_LOOP: for dir in get_script_dirs().iter() {
        let script_path = dir.join(&script_file);

        if let Ok(metadata) = fs::metadata(&script_path) {
            if metadata.is_file() {
                result = Ok(script_path);
                break 'DIR_LOOP;
            }
        }
    }

    result
}
