//! Implements parsing and applying .gitignore files.

use {
    git2,
    glob,
    id_arena::{Arena, Id},
    lazy_static::lazy_static,
    lazy_regex::regex,
    std::{
        fs::File,
        io::{BufRead, BufReader, Result},
        path::{Path, PathBuf},
    },
};

pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// a simple rule of a gitignore file
#[derive(Clone)]
struct GitIgnoreRule {
    ok: bool,        // does this rule when matched means the file is good? (usually false)
    directory: bool, // whether this rule only applies to directories
    filename: bool,  // does this rule apply to just the filename
    pattern: glob::Pattern,
    pattern_options: glob::MatchOptions,
}

impl GitIgnoreRule {
    /// parse a line of a .gitignore file.
    /// The ref_dir is used if the line starts with '/'
    fn from(line: &str, ref_dir: &Path) -> Option<GitIgnoreRule> {
        if line.starts_with('#') {
            return None; // comment line
        }
        let r = regex!(
            r"(?x)
            ^\s*
            (!)?    # 1 : negation
            (.+?)   # 2 : pattern
            (/)?    # 3 : directory
            \s*$
            "
        );
        if let Some(c) = r.captures(line) {
            if let Some(p) = c.get(2) {
                let mut p = p.as_str().to_string();
                let has_separator = p.contains('/');
                if has_separator && p.starts_with('/') {
                    p = ref_dir.to_string_lossy().to_string() + &p;
                }
                if let Ok(pattern) = glob::Pattern::new(&p) {
                    let pattern_options = glob::MatchOptions {
                        case_sensitive: true,
                        require_literal_leading_dot: false,
                        require_literal_separator: has_separator,
                    };
                    return Some(GitIgnoreRule {
                        ok: c.get(1).is_some(), // if negation
                        pattern,
                        directory: c.get(3).is_some(),
                        filename: !has_separator,
                        pattern_options,
                    });
                }
            }
        }
        None
    }
}

/// The rules of a gitignore file
#[derive(Clone)]
pub struct GitIgnoreFile {
    rules: Vec<GitIgnoreRule>,
}
impl GitIgnoreFile {
    /// build a new gitignore file, from either a global ignore file or
    /// a .gitignore file found inside a git repository.
    /// The ref_dir is either:
    /// - the path of the current repository for the global gitignore
    /// - the directory containing the .gitignore file
    pub fn new(file_path: &Path, ref_dir: &Path) -> Result<GitIgnoreFile> {
        let f = File::open(file_path)?;
        let mut rules: Vec<GitIgnoreRule> = Vec::new();
        for line in BufReader::new(f).lines() {
            if let Some(rule) = GitIgnoreRule::from(&line?, ref_dir) {
                rules.push(rule);
            }
        }
        // the last rule applicable to a path is the right one. So
        // we reverse the list to easily iterate from the last one to the first one
        rules.reverse();
        Ok(GitIgnoreFile { rules })
    }
    /// return the global gitignore file interpreted for
    /// the given repo dir
    pub fn global(repo_dir: &Path) -> Option<GitIgnoreFile> {
        lazy_static! {
            static ref GLOBAL_GI_PATH: Option<PathBuf> = find_global_ignore();
        }
        if let Some(path) = &*GLOBAL_GI_PATH {
            GitIgnoreFile::new(path, repo_dir).ok()
        } else {
            None
        }
    }
}

pub fn find_global_ignore() -> Option<PathBuf> {
    git2::Config::open_default()
        .and_then(|global_config| global_config.get_path("core.excludesfile"))
        .ok()
        .or_else(|| {
            directories::BaseDirs::new().map(|base_dirs| base_dirs.config_dir().join("git/ignore"))
        })
        .or_else(|| {
            directories::UserDirs::new()
                .map(|user_dirs| user_dirs.home_dir().join(".config/git/ignore"))
        })
}

#[derive(Debug, Clone, Default)]
pub struct GitIgnoreChain {
    in_repo: bool,
    file_ids: Vec<Id<GitIgnoreFile>>,
}
impl GitIgnoreChain {
    pub fn push(&mut self, id: Id<GitIgnoreFile>) {
        self.file_ids.push(id);
    }
}

#[derive(Default)]
pub struct GitIgnorer {
    files: Arena<GitIgnoreFile>,
}

impl GitIgnorer {
    pub fn root_chain(&mut self, mut dir: &Path) -> GitIgnoreChain {
        let mut chain = GitIgnoreChain::default();
        loop {
            let ignore_file = dir.join(".gitignore");
            let is_repo = is_repo(dir);
            if is_repo {
                if let Some(gif) = GitIgnoreFile::global(dir) {
                    chain.push(self.files.alloc(gif));
                }
            }
            if let Ok(gif) = GitIgnoreFile::new(&ignore_file, dir) {
                chain.push(self.files.alloc(gif));
            }
            if is_repo {
                chain.in_repo = true;
                break;
            }
            if let Some(parent) = dir.parent() {
                dir = parent;
            } else {
                break;
            }
        }
        chain
    }
    pub fn deeper_chain(&mut self, parent_chain: &GitIgnoreChain, dir: &Path) -> GitIgnoreChain {
        // if the current folder is a repository, then
        // we reset the chain to the root one:
        // we don't want the .gitignore files of super repositories
        // (see https://github.com/Canop/broot/issues/160)
        let mut chain = if is_repo(dir) {
            let mut chain = GitIgnoreChain::default();
            if let Some(gif) = GitIgnoreFile::global(dir) {
                chain.push(self.files.alloc(gif));
            }
            chain.in_repo = true;
            chain
        } else {
            parent_chain.clone()
        };
        if chain.in_repo {
            let ignore_file = dir.join(".gitignore");
            if let Ok(gif) = GitIgnoreFile::new(&ignore_file, dir) {
                chain.push(self.files.alloc(gif));
            }
        }
        chain
    }
    /// return true if the given path should not be ignored
    pub fn accepts(
        &self,
        chain: &GitIgnoreChain,
        path: &Path,
        filename: &str,
        directory: bool,
    ) -> bool {
        if !chain.in_repo {
            // if we're not in a git repository, then .gitignore files, including
            // the global ones, are irrelevant
            return true;
        }
        // we start with deeper files: deeper rules have a bigger priority
        for id in chain.file_ids.iter().rev() {
            let file = &self.files[*id];
            for rule in &file.rules {
                if rule.directory && !directory {
                    continue;
                }
                let ok = if rule.filename {
                    rule.pattern.matches_with(filename, rule.pattern_options)
                } else {
                    rule.pattern.matches_path_with(path, rule.pattern_options)
                };
                if ok {
                    return rule.ok;
                }
            }
        }
        true
    }
}
