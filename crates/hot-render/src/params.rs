//! Parameters in override sources: `{{name}}` or `{{name=default}}`
//! anywhere in the WGSL is replaced by the parameter's value as a float
//! literal before the source is checked and compiled. Changing a value
//! (`render.param`) recompiles the overrides that use it, nothing else.

/// One `{{...}}` in a source.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub name: String,
    pub default: Option<f32>,
}

/// Whether `name` is a parameter name: `[A-Za-z0-9_.]{1,64}`.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// Every `{{...}}` in `source`, in order (a name may repeat).
pub fn scan(source: &str) -> Result<Vec<Use>, String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| "`{{` without a closing `}}`".to_string())?;
        out.push(parse_use(after[..end].trim())?);
        rest = &after[end + 2..];
    }
    Ok(out)
}

fn parse_use(body: &str) -> Result<Use, String> {
    let (name, default) = match body.split_once('=') {
        Some((n, d)) => {
            let d = d.trim();
            let v: f32 = d
                .parse()
                .map_err(|_| format!("{{{{{body}}}}}: default {d:?} is not a number"))?;
            if !v.is_finite() {
                return Err(format!("{{{{{body}}}}}: default is not finite"));
            }
            (n.trim(), Some(v))
        }
        None => (body, None),
    };
    if !valid_name(name) {
        return Err(format!(
            "{{{{{body}}}}}: a parameter name is letters, digits, `_` and `.` (at most 64)"
        ));
    }
    Ok(Use {
        name: name.to_owned(),
        default,
    })
}

/// A WGSL float literal for `v` (parenthesised when negative, so it can
/// follow any operator).
pub fn literal(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let text = if v.fract() == 0.0 && v.abs() < 1e9 {
        format!("{v:.1}")
    } else {
        format!("{v:?}")
    };
    if v.is_sign_negative() && v != 0.0 {
        format!("({text})")
    } else {
        text
    }
}

/// `source` with every `{{...}}` replaced by `value(name)` (the default,
/// then 0, when it has none).
pub fn substitute(source: &str, value: impl Fn(&str) -> Option<f32>) -> Result<String, String> {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| "`{{` without a closing `}}`".to_string())?;
        let u = parse_use(after[..end].trim())?;
        let v = value(&u.name).or(u.default).unwrap_or(0.0);
        out.push_str(&literal(v));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_names_and_defaults() {
        let u = scan("let a = {{gain=1.5}} * {{ bias }} + {{gain}};").unwrap();
        assert_eq!(
            u,
            vec![
                Use {
                    name: "gain".into(),
                    default: Some(1.5)
                },
                Use {
                    name: "bias".into(),
                    default: None
                },
                Use {
                    name: "gain".into(),
                    default: None
                },
            ]
        );
        assert!(scan("{{gain").is_err());
        assert!(scan("{{a b}}").is_err());
        assert!(scan("{{a=x}}").is_err());
        assert!(scan("{{a=inf}}").is_err());
    }

    #[test]
    fn literals_are_wgsl_floats() {
        assert_eq!(literal(1.0), "1.0");
        assert_eq!(literal(0.25), "0.25");
        assert_eq!(literal(-2.0), "(-2.0)");
        assert_eq!(literal(f32::NAN), "0.0");
        assert_eq!(literal(1e-7), "1e-7");
    }

    #[test]
    fn substitutes_values_then_defaults_then_zero() {
        let s = substitute("x * {{gain=2}} + {{bias}} - {{set=1}}", |n| {
            (n == "set").then_some(0.5)
        })
        .unwrap();
        assert_eq!(s, "x * 2.0 + 0.0 - 0.5");
    }
}
