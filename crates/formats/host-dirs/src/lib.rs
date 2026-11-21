//! The per-user `h2recomp` directories: data, cache and config, under the
//! convention of the host (docs/runtime/storage.md, "Host directories").
//!
//! Each directory is one function of a [`Convention`] and an environment
//! lookup, so every convention is tested on every host. [`Convention::HOST`]
//! is the only place the target is consulted; [`data`], [`cache`] and
//! [`config`] apply it to the process environment.
//!
//! A variable that is unset, empty or a relative path counts as unset. A
//! convention whose variable is missing has no directory (`None`); Windows
//! never falls back to `HOME`. A UWP host has no such directories: it
//! passes its package's LocalFolder to `vfs::Config::new` instead.
use std::ffi::OsString;
use std::path::PathBuf;

/// The leaf every directory ends in.
pub const APP: &str = "h2recomp";

/// Which per-user directory convention a host follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convention {
    /// macOS, iOS and tvOS: `~/Library/Application Support` and
    /// `~/Library/Caches`.
    Apple,
    /// `%LOCALAPPDATA%`.
    Windows,
    /// The XDG base directories, with their `~/.local/share`, `~/.cache` and
    /// `~/.config` defaults. Every Unix that is not Apple.
    Xdg,
}

impl Convention {
    /// The convention of the host this binary is built for: Apple targets
    /// and Windows are named, every other target is XDG.
    pub const HOST: Self = if cfg!(target_vendor = "apple") {
        Self::Apple
    } else if cfg!(windows) {
        Self::Windows
    } else {
        Self::Xdg
    };
}

/// The value of `name` as an absolute path; `None` when it is unset, empty
/// or relative.
fn absolute(env: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `$<xdg>`, else `$HOME/<fallback>`.
fn xdg(env: &impl Fn(&str) -> Option<OsString>, var: &str, fallback: &str) -> Option<PathBuf> {
    absolute(env, var).or_else(|| Some(absolute(env, "HOME")?.join(fallback)))
}

/// The application data directory: Apple
/// `~/Library/Application Support/h2recomp`, Windows
/// `%LOCALAPPDATA%\h2recomp`, XDG `$XDG_DATA_HOME/h2recomp`, else
/// `~/.local/share/h2recomp`.
pub fn data_dir(convention: Convention, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = match convention {
        Convention::Apple => absolute(&env, "HOME")?.join("Library/Application Support"),
        Convention::Windows => absolute(&env, "LOCALAPPDATA")?,
        Convention::Xdg => xdg(&env, "XDG_DATA_HOME", ".local/share")?,
    };
    Some(base.join(APP))
}

/// The cache directory: Apple `~/Library/Caches/h2recomp`, Windows
/// `%LOCALAPPDATA%\h2recomp\cache`, XDG `$XDG_CACHE_HOME/h2recomp`, else
/// `~/.cache/h2recomp`.
pub fn cache_dir(
    convention: Convention,
    env: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    Some(match convention {
        Convention::Apple => absolute(&env, "HOME")?.join("Library/Caches").join(APP),
        Convention::Windows => absolute(&env, "LOCALAPPDATA")?.join(APP).join("cache"),
        Convention::Xdg => xdg(&env, "XDG_CACHE_HOME", ".cache")?.join(APP),
    })
}

/// The config directory: Apple `~/Library/Application Support/h2recomp`
/// and Windows `%LOCALAPPDATA%\h2recomp`, both the same as [`data_dir`];
/// XDG `$XDG_CONFIG_HOME/h2recomp`, else `~/.config/h2recomp`.
pub fn config_dir(
    convention: Convention,
    env: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    match convention {
        Convention::Apple | Convention::Windows => data_dir(convention, env),
        Convention::Xdg => Some(xdg(&env, "XDG_CONFIG_HOME", ".config")?.join(APP)),
    }
}

/// [`data_dir`] for this host and process.
pub fn data() -> Option<PathBuf> {
    data_dir(Convention::HOST, |name| std::env::var_os(name))
}

/// [`cache_dir`] for this host and process.
pub fn cache() -> Option<PathBuf> {
    cache_dir(Convention::HOST, |name| std::env::var_os(name))
}

/// [`config_dir`] for this host and process.
pub fn config() -> Option<PathBuf> {
    config_dir(Convention::HOST, |name| std::env::var_os(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Convention::{Apple, Windows, Xdg};

    type Env = Box<dyn Fn(&str) -> Option<OsString>>;

    #[derive(Debug, Clone, Copy)]
    enum Dir {
        Data,
        Cache,
        Config,
    }
    use Dir::{Cache, Config, Data};

    impl Dir {
        fn of(self, convention: Convention, env: &Env) -> Option<PathBuf> {
            match self {
                Data => data_dir(convention, env),
                Cache => cache_dir(convention, env),
                Config => config_dir(convention, env),
            }
        }
    }

    /// An absolute path on the host running the test: Windows needs a drive.
    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:{p}")
        } else {
            p.to_owned()
        }
    }

    fn vars(pairs: &[(&'static str, &str)]) -> Env {
        let pairs: Vec<(&'static str, OsString)> =
            pairs.iter().map(|(k, v)| (*k, OsString::from(v))).collect();
        Box::new(move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        })
    }

    /// `abs(base)` joined with each of `rest`.
    fn path(base: &str, rest: &[&str]) -> Option<PathBuf> {
        Some(rest.iter().fold(PathBuf::from(abs(base)), |p, r| p.join(r)))
    }

    /// Every variable set, each to its own directory.
    fn all() -> Env {
        vars(&[
            ("HOME", &*abs("/h")),
            ("LOCALAPPDATA", &*abs("/l")),
            ("XDG_DATA_HOME", &*abs("/xd")),
            ("XDG_CACHE_HOME", &*abs("/xc")),
            ("XDG_CONFIG_HOME", &*abs("/xf")),
        ])
    }

    /// Only `HOME`.
    fn home() -> Env {
        vars(&[("HOME", &*abs("/h"))])
    }

    #[test]
    fn every_directory_under_every_convention() {
        let (all, home) = (all(), home());
        let table = [
            (
                Data,
                Apple,
                &all,
                path("/h", &["Library/Application Support", APP]),
            ),
            (Data, Windows, &all, path("/l", &[APP])),
            (Data, Xdg, &all, path("/xd", &[APP])),
            (Data, Xdg, &home, path("/h", &[".local/share", APP])),
            (Cache, Apple, &all, path("/h", &["Library/Caches", APP])),
            (Cache, Windows, &all, path("/l", &[APP, "cache"])),
            (Cache, Xdg, &all, path("/xc", &[APP])),
            (Cache, Xdg, &home, path("/h", &[".cache", APP])),
            (
                Config,
                Apple,
                &all,
                path("/h", &["Library/Application Support", APP]),
            ),
            (Config, Windows, &all, path("/l", &[APP])),
            (Config, Xdg, &all, path("/xf", &[APP])),
            (Config, Xdg, &home, path("/h", &[".config", APP])),
            // Windows never falls back to HOME.
            (Data, Windows, &home, None),
            (Cache, Windows, &home, None),
            (Config, Windows, &home, None),
        ];
        for (i, (dir, convention, env, want)) in table.into_iter().enumerate() {
            assert_eq!(
                dir.of(convention, env),
                want,
                "row {i}: {dir:?} under {convention:?}"
            );
        }
    }

    #[test]
    fn the_byte_exact_apple_and_windows_paths() {
        // Saves, caches and lab peers already live at these exact strings.
        if !cfg!(windows) {
            let env = vars(&[("HOME", "/Users/u")]);
            let s = |dir: Dir| dir.of(Apple, &env).unwrap().into_os_string();
            assert_eq!(s(Data), "/Users/u/Library/Application Support/h2recomp");
            assert_eq!(s(Cache), "/Users/u/Library/Caches/h2recomp");
            assert_eq!(s(Config), "/Users/u/Library/Application Support/h2recomp");
        }
        let env = vars(&[("LOCALAPPDATA", &*abs("/l"))]);
        let local = PathBuf::from(abs("/l"));
        assert_eq!(Data.of(Windows, &env), Some(local.join("h2recomp")));
        assert_eq!(Config.of(Windows, &env), Some(local.join("h2recomp")));
        assert_eq!(
            Cache.of(Windows, &env),
            Some(local.join("h2recomp").join("cache"))
        );
    }

    #[test]
    fn empty_relative_and_missing_variables_count_as_unset() {
        let names = [
            "HOME",
            "LOCALAPPDATA",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_CONFIG_HOME",
        ];
        let empty: Vec<_> = names.iter().map(|n| (*n, "")).collect();
        let relative: Vec<_> = names.iter().map(|n| (*n, "rel/dir")).collect();
        for env in [vars(&[]), vars(&empty), vars(&relative)] {
            for dir in [Data, Cache, Config] {
                for convention in [Apple, Windows, Xdg] {
                    assert_eq!(
                        dir.of(convention, &env),
                        None,
                        "{dir:?} under {convention:?}"
                    );
                }
            }
        }
        // An empty or relative XDG variable falls back to HOME.
        let env = vars(&[
            ("HOME", &*abs("/h")),
            ("XDG_DATA_HOME", ""),
            ("XDG_CACHE_HOME", "rel"),
            ("XDG_CONFIG_HOME", ""),
        ]);
        assert_eq!(Data.of(Xdg, &env), path("/h", &[".local/share", APP]));
        assert_eq!(Cache.of(Xdg, &env), path("/h", &[".cache", APP]));
        assert_eq!(Config.of(Xdg, &env), path("/h", &[".config", APP]));
    }

    #[test]
    fn the_host_convention() {
        let want = if cfg!(target_vendor = "apple") {
            Apple
        } else if cfg!(windows) {
            Windows
        } else {
            Xdg
        };
        assert_eq!(Convention::HOST, want);
    }
}
