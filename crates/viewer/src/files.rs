//! Finding the other layers next to an opened image.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["jp2", "j2k", "j2c", "jph", "jhc", "jpc"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Compare names so `layer_2` sorts before `layer_10`, ignoring case.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(&c) = it.peek() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut a), num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// All images in `dir`, naturally sorted.
pub fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    v.sort_by(|a, b| {
        natural_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
    v
}

/// Resolve what the user opened (a file or a folder) into the layer list and the index to show.
pub fn resolve(path: &Path) -> Result<(Vec<PathBuf>, usize), String> {
    if path.is_dir() {
        let files = list_images(path);
        if files.is_empty() {
            return Err(format!("no JPEG 2000 files in {}", path.display()));
        }
        return Ok((files, 0));
    }
    if !path.is_file() {
        return Err(format!("{}: not found", path.display()));
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut files = list_images(dir);
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let want = canon(path);
    match files.iter().position(|f| canon(f) == want) {
        Some(i) => Ok((files, i)),
        None => {
            // Unusual extension: still open it, as the only layer plus its siblings.
            files.insert(0, path.to_path_buf());
            Ok((files, 0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec![
            "layer_10.jp2",
            "layer_2.jp2",
            "Layer_1.jp2",
            "layer_9.jp2",
            "layer_011.jp2",
        ];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            [
                "Layer_1.jp2",
                "layer_2.jp2",
                "layer_9.jp2",
                "layer_10.jp2",
                "layer_011.jp2"
            ]
        );
    }

    #[test]
    fn prefix_sorts_first_and_equal_is_equal() {
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("A7", "a7"), Ordering::Equal);
        assert_eq!(natural_cmp("a007", "a7"), Ordering::Equal);
    }

    #[test]
    fn extensions() {
        assert!(is_image(Path::new("x/y/layer_1.JP2")));
        assert!(is_image(Path::new("a.j2c")));
        assert!(!is_image(Path::new("a.png")));
        assert!(!is_image(Path::new("jp2")));
    }
}
