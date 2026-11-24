//! `strings.list`: every `unic` string of the open map by name and
//! language, searched (docs/editor/rich-editors.md, "Other editors worth
//! building", slice N1). A read of `tags::Localization`: the text lives in
//! the cache's per-language tables, and each `unic` tag selects a span of
//! them per language.

use agent_proto::RpcError;
use map::{FourCC, TagId};
use serde_json::{Map as Object, Value, json};
use studio_proto::{code, method as m};
use tags::localization::{Language, Localization, StringId, Unic};

use super::scenario::tag_id;
use super::tags::{open_map, resolve};
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;

const LIST: MethodDef = def(
    m::STRINGS_LIST,
    "Every unic string of the open map by name and language, searched by name or text",
    r#"{"tag?":"string|integer (one unic tag)","language?":"string (en ja de fr es it ko zh pt)","query?":"string (case-insensitive, in the name, the id or any shown text)","limit?":"integer (default 200, max 5000)","offset?":"integer"}"#,
    r#"{"languages":"[string]","tags":"[{tag,name,counts:{language:integer}}]","total":"integer","offset":"integer","limit":"integer","truncated":"boolean","strings":"[{tag,tag_name,id,name,texts:{language:string}}]","problems":"[string]","unavailable?":"string"}"#,
    "{}",
);

/// The most rows one call returns.
const MAX_LIMIT: usize = 5000;
const DEFAULT_LIMIT: usize = 200;

pub(super) struct Strings;

impl Provider for Strings {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        match method {
            m::STRINGS_LIST => Some(list(app, &P(params))),
            _ => None,
        }
    }
}

/// One `unic` tag to list.
struct Source {
    tag: String,
    name: Option<String>,
    unic: Unic,
}

/// Which of the strings to show.
struct Filter {
    /// One language, or all nine.
    language: Option<Language>,
    /// Lower case; matches a name, an id or any shown text.
    query: Option<String>,
    limit: usize,
    offset: usize,
}

fn list(app: &App, p: &P) -> Reply {
    let language = p
        .opt_str("language")?
        .map(|s| {
            s.parse::<Language>()
                .map_err(|e| err(code::INVALID_PARAMS, format!("language: {e}")))
        })
        .transpose()?;
    let limit = p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(err(
            code::INVALID_PARAMS,
            format!("limit: 1..{MAX_LIMIT}, not {limit}"),
        ));
    }
    let filter = Filter {
        language,
        query: p.opt_str("query")?.map(str::to_lowercase),
        limit,
        offset: p.opt_usize("offset")?.unwrap_or(0),
    };
    let mut map = open_map(app)?;
    let unic = FourCC(*b"unic");
    let want = match p.get("tag") {
        Some(v) => {
            let t = resolve(&map, v)?;
            if t.group != unic {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("tag: {} is not a unic tag", tag_id(t.id)),
                ));
            }
            Some(t.id)
        }
        None => None,
    };
    let tags: Vec<(TagId, Option<String>)> = map
        .tags()
        .iter()
        .filter(|t| t.group == unic && want.is_none_or(|w| w == t.id))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let loc = match Localization::read(&mut map) {
        Ok(l) => l,
        // A map without locale tables (a converted or invented one) has
        // no strings: say why, with the same shape.
        Err(e) => {
            let mut v = rows(
                &Localization::from_tables(std::array::from_fn(|_| {
                    tags::localization::LocaleTable::decode(&[], Vec::new()).expect("empty table")
                })),
                &[],
                &[],
                &filter,
            );
            v["unavailable"] = json!(format!("the map's localization is not readable: {e}"));
            return Ok(v);
        }
    };
    let mut sources = Vec::new();
    let mut problems = Vec::new();
    for (id, name) in tags {
        match loc.unic(&mut map, id) {
            Ok(unic) => sources.push(Source {
                tag: tag_id(id),
                name,
                unic,
            }),
            Err(e) => problems.push(format!("{}: {e}", tag_id(id))),
        }
    }
    let mut v = rows(&loc, map.strings(), &sources, &filter);
    if !problems.is_empty() {
        v["problems"]
            .as_array_mut()
            .expect("problems")
            .splice(0..0, problems.into_iter().map(Value::from));
    }
    Ok(v)
}

fn id_name(id: StringId, names: &[Option<String>]) -> Option<String> {
    id.resolve(names).ok().flatten().map(str::to_owned)
}

/// The listing: one row per string key of each tag, in the order the key
/// first appears across the shown languages, with its text in each.
fn rows(loc: &Localization, names: &[Option<String>], sources: &[Source], f: &Filter) -> Value {
    let languages: Vec<Language> = match f.language {
        Some(l) => vec![l],
        None => Language::ALL.to_vec(),
    };
    let mut tag_rows = Vec::new();
    let mut matched = Vec::new();
    let mut problems = Vec::new();
    for s in sources {
        let mut counts = Object::new();
        let mut keys: Vec<StringId> = Vec::new();
        let mut shown: Vec<Language> = Vec::new();
        for &lang in &languages {
            match loc.entries_for(&s.unic, lang) {
                Ok(entries) => {
                    counts.insert(lang.code().into(), json!(entries.len()));
                    shown.push(lang);
                    for e in entries {
                        if !keys.contains(&e.id) {
                            keys.push(e.id);
                        }
                    }
                }
                Err(e) => problems.push(format!("{} {}: {e}", s.tag, lang.code())),
            }
        }
        tag_rows.push(json!({ "tag": s.tag, "name": s.name, "counts": counts }));
        for id in keys {
            let mut texts = Object::new();
            for &lang in &shown {
                if let Ok(Some(text)) = loc.text_in(&s.unic, id, lang) {
                    texts.insert(lang.code().into(), json!(text));
                }
            }
            let name = id_name(id, names);
            let hex = format!("{:#010x}", id.0);
            if let Some(q) = &f.query {
                let hit = name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(q))
                    || hex.contains(q.as_str())
                    || texts
                        .values()
                        .any(|t| t.as_str().is_some_and(|t| t.to_lowercase().contains(q)));
                if !hit {
                    continue;
                }
            }
            matched.push(json!({
                "tag": s.tag, "tag_name": s.name, "id": hex, "name": name, "texts": texts,
            }));
        }
    }
    let total = matched.len();
    let page: Vec<Value> = matched.into_iter().skip(f.offset).take(f.limit).collect();
    json!({
        "languages": Language::ALL.iter().map(|l| l.code()).collect::<Vec<_>>(),
        "tags": tag_rows,
        "total": total,
        "offset": f.offset,
        "limit": f.limit,
        "truncated": f.offset + page.len() < total,
        "strings": page,
        "problems": problems,
    })
}

#[cfg(test)]
mod tests {
    use tags::localization::{LanguageRange, LocaleTable};

    use super::*;
    use crate::api::Api;

    /// A table of (id, text) rows, texts laid out one after another.
    fn table(rows: &[(u32, &str)]) -> LocaleTable {
        let mut index = Vec::new();
        let mut blob = Vec::new();
        for (id, text) in rows {
            index.extend(id.to_le_bytes());
            index.extend((blob.len() as u32).to_le_bytes());
            blob.extend(text.as_bytes());
            blob.push(0);
        }
        LocaleTable::decode(&index, blob).unwrap()
    }

    /// The names the keys are made from: an id is the name's length over
    /// the index.
    fn names() -> Vec<Option<String>> {
        ["", "greeting", "farewell", "title"]
            .map(|n| Some(n.to_owned()))
            .to_vec()
    }
    const GREETING: u32 = 0x0800_0001;
    const FAREWELL: u32 = 0x0800_0002;
    const TITLE: u32 = 0x0500_0003;

    /// Invented: English has three strings, Japanese and German two, the
    /// rest none.
    fn localization() -> Localization {
        let mut tables: [LocaleTable; 9] = std::array::from_fn(|_| table(&[]));
        tables[Language::English as usize] = table(&[
            (GREETING, "Hello"),
            (FAREWELL, "Goodbye"),
            (TITLE, "Welcome"),
        ]);
        tables[Language::Japanese as usize] =
            table(&[(GREETING, "こんにちは"), (TITLE, "ようこそ")]);
        tables[Language::German as usize] =
            table(&[(GREETING, "Hallo"), (FAREWELL, "Auf Wiedersehen")]);
        Localization::from_tables(tables)
    }

    fn unic(spans: &[(Language, i16, i16)]) -> Unic {
        let mut u = Unic::decode(&[0; Unic::SIZE]).unwrap();
        for &(l, start, count) in spans {
            u.ranges[l as usize] = LanguageRange { start, count };
        }
        u
    }

    fn sources() -> Vec<Source> {
        vec![
            Source {
                tag: "0xe0000001".into(),
                name: Some("ui\\menu".into()),
                unic: unic(&[
                    (Language::English, 0, 3),
                    (Language::Japanese, 0, 2),
                    (Language::German, 0, 2),
                ]),
            },
            // Only the English title, from the middle of its table.
            Source {
                tag: "0xe0000002".into(),
                name: Some("ui\\title".into()),
                unic: unic(&[(Language::English, 2, 1)]),
            },
        ]
    }

    fn filter(language: Option<Language>, query: Option<&str>) -> Filter {
        Filter {
            language,
            query: query.map(str::to_lowercase),
            limit: 200,
            offset: 0,
        }
    }

    fn strings(v: &Value) -> &Vec<Value> {
        v["strings"].as_array().unwrap()
    }

    #[test]
    fn lists_every_string_by_name_with_its_text_in_each_language() {
        let v = rows(&localization(), &names(), &sources(), &filter(None, None));
        assert_eq!(v["languages"].as_array().unwrap().len(), 9);
        assert_eq!(v["total"], 4);
        assert_eq!(v["truncated"], false);
        let rows = strings(&v);
        let greeting = &rows[0];
        assert_eq!(greeting["name"], "greeting");
        assert_eq!(greeting["id"], "0x08000001");
        assert_eq!(greeting["tag_name"], "ui\\menu");
        assert_eq!(
            greeting["texts"],
            json!({"en": "Hello", "ja": "こんにちは", "de": "Hallo"})
        );
        // A language without the key has no text, not an empty one.
        assert_eq!(rows[1]["name"], "farewell");
        assert_eq!(
            rows[1]["texts"],
            json!({"en": "Goodbye", "de": "Auf Wiedersehen"})
        );
        assert_eq!(rows[2]["texts"], json!({"en": "Welcome", "ja": "ようこそ"}));
        // The second tag selects its span only.
        assert_eq!(rows[3]["tag"], "0xe0000002");
        assert_eq!(rows[3]["texts"], json!({"en": "Welcome"}));
        let tag = &v["tags"][0];
        assert_eq!(
            tag["counts"],
            json!({"en": 3, "ja": 2, "de": 2, "fr": 0, "es": 0, "it": 0, "ko": 0, "zh": 0, "pt": 0})
        );
    }

    #[test]
    fn one_language_lists_only_its_strings() {
        let l = localization();
        let ja = rows(
            &l,
            &names(),
            &sources(),
            &filter(Some(Language::Japanese), None),
        );
        assert_eq!(ja["total"], 2);
        assert_eq!(strings(&ja)[0]["texts"], json!({"ja": "こんにちは"}));
        assert_eq!(strings(&ja)[1]["name"], "title");
        let de = rows(
            &l,
            &names(),
            &sources(),
            &filter(Some(Language::German), None),
        );
        assert_eq!(
            strings(&de)
                .iter()
                .map(|r| r["name"].clone())
                .collect::<Vec<_>>(),
            [json!("greeting"), json!("farewell")]
        );
        // A language the tags have nothing in lists nothing.
        let fr = rows(
            &l,
            &names(),
            &sources(),
            &filter(Some(Language::French), None),
        );
        assert_eq!(fr["total"], 0);
    }

    #[test]
    fn search_matches_a_name_an_id_or_the_text_in_any_shown_language() {
        let l = localization();
        let by = |lang, q| rows(&l, &names(), &sources(), &filter(lang, Some(q)));
        // By name, case-insensitively.
        let r = by(None, "FARE");
        assert_eq!(r["total"], 1);
        assert_eq!(strings(&r)[0]["name"], "farewell");
        // By the German text, which only the German column has.
        let r = by(None, "wiedersehen");
        assert_eq!(r["total"], 1);
        assert_eq!(strings(&r)[0]["name"], "farewell");
        // By Japanese text.
        assert_eq!(by(None, "ようこそ")["total"], 1);
        // By id.
        assert_eq!(by(None, "0x08000001")["total"], 1);
        // A text in a language that is not shown does not match.
        assert_eq!(by(Some(Language::English), "hallo")["total"], 0);
        assert_eq!(by(Some(Language::German), "hallo")["total"], 1);
        // A miss is an empty list, not an error.
        assert_eq!(by(None, "nothing like this")["total"], 0);
    }

    #[test]
    fn a_page_says_when_there_is_more() {
        let mut f = filter(None, None);
        f.limit = 3;
        let v = rows(&localization(), &names(), &sources(), &f);
        assert_eq!(
            (
                v["total"].clone(),
                strings(&v).len(),
                v["truncated"].clone()
            ),
            (json!(4), 3, json!(true))
        );
        f.offset = 3;
        let v = rows(&localization(), &names(), &sources(), &f);
        assert_eq!(
            (strings(&v).len(), v["truncated"].clone()),
            (1, json!(false))
        );
    }

    #[test]
    fn an_unnamed_id_lists_by_its_value_and_a_bad_range_is_a_problem() {
        let mut s = sources();
        // The name list is shorter than the ids' indices.
        let short = vec![Some(String::new())];
        let v = rows(&localization(), &short, &s, &filter(None, None));
        assert_eq!(strings(&v)[0]["name"], Value::Null);
        assert_eq!(strings(&v)[0]["id"], "0x08000001");
        // A range past the table is named, and the other languages stay.
        s[0].unic.ranges[Language::German as usize] = LanguageRange { start: 1, count: 9 };
        let v = rows(&localization(), &names(), &s, &filter(None, None));
        let problems = v["problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].as_str().unwrap().starts_with("0xe0000001 de:"));
        assert_eq!(
            strings(&v)[0]["texts"],
            json!({"en": "Hello", "ja": "こんにちは"})
        );
    }

    #[test]
    fn the_method_checks_its_params_and_needs_a_map() {
        let mut app = App::new();
        let mut api = Api::new();
        let bad = api
            .call(&mut app, m::STRINGS_LIST, json!({"language": "xx"}))
            .unwrap_err();
        assert_eq!(bad.code, code::INVALID_PARAMS);
        let bad = api
            .call(&mut app, m::STRINGS_LIST, json!({"limit": 0}))
            .unwrap_err();
        assert_eq!(bad.code, code::INVALID_PARAMS);
        let none = api.call(&mut app, m::STRINGS_LIST, json!({})).unwrap_err();
        assert_eq!(none.code, code::NO_DOCUMENT);
    }
}
