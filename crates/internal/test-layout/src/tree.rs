//! The walk from crate roots through every module they declare.

use std::collections::{HashSet, VecDeque};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    Breach, Sources, Violation, layout,
    scan::{self, Report, Role},
};

/// Check every file reached from `roots`, read from `sources`.
///
/// Each root is a crate root.
/// Returns the violations sorted by file, then line.
pub fn check<S: Sources + ?Sized>(
    roots: impl IntoIterator<Item = Utf8PathBuf>,
    sources: &S,
) -> Vec<Violation> {
    let mut queue: VecDeque<(Utf8PathBuf, bool, Role)> = roots
        .into_iter()
        .map(|root| (root, true, Role::Production))
        .collect();
    let mut seen = HashSet::new();
    let mut violations = Vec::new();

    while let Some((path, owns_dir, role)) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }

        let report = match scan_file(&path, owns_dir, role, sources) {
            Ok(report) => report,
            Err((line, breach)) => {
                violations.push(Violation {
                    file: path,
                    line,
                    breach,
                });
                continue;
            }
        };

        for (line, breach) in report.breaches {
            violations.push(Violation {
                file: path.clone(),
                line,
                breach,
            });
        }

        for (decl, role) in report.children {
            let found = layout::candidates(&path, owns_dir, &decl)
                .into_iter()
                .find(|(candidate, _)| sources.exists(candidate));
            match found {
                Some((child, owns_dir)) => queue.push_back((child, owns_dir, role)),
                None => violations.push(Violation {
                    file: path.clone(),
                    line: decl.line,
                    breach: Breach::MissingModule { name: decl.name },
                }),
            }
        }
    }

    violations.sort();
    violations
}

fn scan_file<S: Sources + ?Sized>(
    path: &Utf8Path,
    owns_dir: bool,
    role: Role,
    sources: &S,
) -> Result<Report, (usize, Breach)> {
    let source = sources.read(path).map_err(|error| {
        (1, Breach::Unreadable {
            error: error.to_string(),
        })
    })?;
    let file = syn::parse_file(&source).map_err(|error| {
        (error.span().start().line, Breach::Unparsable {
            error: error.to_string(),
        })
    })?;

    Ok(match role {
        Role::Production => scan::production(path, owns_dir, &file),
        Role::Tests(form) => scan::tests(path, &form, &file),
    })
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
