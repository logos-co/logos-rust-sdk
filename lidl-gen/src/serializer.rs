//! ModuleDecl → `.lidl` text — the Rust analog of logos-lidl's C++
//! `lidlSerialize`, so a Rust-declared contract (see rust_frontend) can be
//! published as the same language-neutral interchange every other backend
//! consumes.

use crate::ast::*;

fn type_text(ty: &TypeExpr) -> String {
    match ty.kind {
        TypeKind::Primitive | TypeKind::Named => ty.name.clone(),
        TypeKind::Array => format!(
            "[{}]",
            ty.elements.first().map(type_text).unwrap_or_default()
        ),
        TypeKind::Map => {
            let k = ty.elements.first().map(type_text).unwrap_or_default();
            let v = ty.elements.get(1).map(type_text).unwrap_or_default();
            format!("{{{}: {}}}", k, v)
        }
        TypeKind::Optional => format!(
            "? {}",
            ty.elements.first().map(type_text).unwrap_or_default()
        ),
    }
}

fn params_text(params: &[ParamDecl]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_text(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Serialize a ModuleDecl as `.lidl` source.
pub fn serialize(module: &ModuleDecl) -> String {
    let mut out = String::new();
    out.push_str(&format!("module {} {{\n", module.name));
    if !module.version.is_empty() {
        out.push_str(&format!("  version \"{}\"\n", module.version));
    }
    if !module.description.is_empty() {
        out.push_str(&format!("  description \"{}\"\n", module.description));
    }
    if !module.category.is_empty() {
        out.push_str(&format!("  category \"{}\"\n", module.category));
    }
    if !module.depends.is_empty() {
        out.push_str(&format!("  depends [{}]\n", module.depends.join(", ")));
    }
    for t in &module.types {
        out.push_str(&format!("  type {} {{\n", t.name));
        for f in &t.fields {
            out.push_str(&format!(
                "    {}{}: {}\n",
                if f.optional { "? " } else { "" },
                f.name,
                type_text(&f.ty)
            ));
        }
        out.push_str("  }\n");
    }
    for m in &module.methods {
        out.push_str(&format!(
            "  method {}({}) -> {}\n",
            m.name,
            params_text(&m.params),
            type_text(&m.return_type)
        ));
    }
    for e in &module.events {
        out.push_str(&format!(
            "  event {}({})\n",
            e.name,
            params_text(&e.params)
        ));
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn round_trips_through_the_parser() {
        let src = r#"
module rt_module {
  version "2.1.0"
  depends [calc_module, auth_module]
  method add(a: int, b: int) -> int
  method names() -> [tstr]
  method fetch(blob: bstr) -> result
  event ready(version: tstr, count: uint)
}
"#;
        let m1 = parse(src).unwrap();
        let text = serialize(&m1);
        let m2 = parse(&text).unwrap();
        assert_eq!(m1, m2);
    }
}
