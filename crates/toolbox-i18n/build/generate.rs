//! Generate typed, allocation-on-render messages; no parser enters the binary.

use std::fmt::Write;

use crate::catalog::{RustMessage, Syntax, template, variant};

fn expression(text: &str) -> String {
    let parsed = template(text, Syntax::Rust).expect("validated format");
    if parsed.parameters.is_empty() {
        return format!("{:?}.to_owned()", parsed.literal);
    }
    let arguments = parsed
        .parameters
        .iter()
        .map(|name| format!("{name} = {name}"))
        .collect::<Vec<_>>();
    format!("format!({text:?}, {})", arguments.join(", "))
}

pub fn rust(messages: &[RustMessage]) -> String {
    let mut out = String::from("// Generated from locales/rust.pot and the shared Chinese PO.\n");
    out.push_str("#[derive(Clone, Debug, PartialEq, Eq)]\npub enum Message {\n");
    for message in messages {
        let name = variant(&message.id);
        if message.parameters.is_empty() {
            writeln!(out, "    {name},").unwrap();
        } else {
            writeln!(out, "    {name} {{").unwrap();
            for parameter in &message.parameters {
                let ty = if parameter == "count" && message.plural.is_some() {
                    "usize"
                } else {
                    "String"
                };
                writeln!(out, "        {parameter}: {ty},").unwrap();
            }
            out.push_str("    },\n");
        }
    }
    out.push_str("}\n\nimpl Message {\n    pub fn render(&self, __locale: Locale) -> String {\n        match self {\n");
    for message in messages {
        let name = variant(&message.id);
        let fields = if message.parameters.is_empty() {
            String::new()
        } else {
            format!(" {{ {} }}", message.parameters.join(", "))
        };
        writeln!(out, "            Self::{name}{fields} => match __locale {{").unwrap();
        if let Some(plural) = &message.plural {
            writeln!(
                out,
                "                Locale::En => if *count == 1 {{ {} }} else {{ {} }},",
                expression(&message.english),
                expression(plural)
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "                Locale::En => {},",
                expression(&message.english)
            )
            .unwrap();
        }
        writeln!(
            out,
            "                Locale::ZhCn => {},",
            expression(&message.chinese)
        )
        .unwrap();
        out.push_str("            },\n");
    }
    out.push_str("        }\n    }\n}\n\n/// Typed constructors generated from the Rust message template.\npub mod messages {\n    use super::Message;\n");
    for message in messages {
        let name = variant(&message.id);
        let parameters = message
            .parameters
            .iter()
            .map(|parameter| {
                let ty = if parameter == "count" && message.plural.is_some() {
                    "usize"
                } else {
                    "impl std::fmt::Display"
                };
                format!("{parameter}: {ty}")
            })
            .collect::<Vec<_>>();
        if parameters.len() > 7 {
            // A complete translatable sentence can legitimately have many arguments.
            out.push_str("    #[allow(clippy::too_many_arguments)]\n");
        }
        writeln!(
            out,
            "    pub fn {}({}) -> Message {{",
            message.id,
            parameters.join(", ")
        )
        .unwrap();
        if message.parameters.is_empty() {
            writeln!(out, "        Message::{name}").unwrap();
        } else {
            writeln!(out, "        Message::{name} {{").unwrap();
            for parameter in &message.parameters {
                if parameter == "count" && message.plural.is_some() {
                    writeln!(out, "            {parameter},").unwrap();
                } else {
                    writeln!(out, "            {parameter}: {parameter}.to_string(),").unwrap();
                }
            }
            out.push_str("        }\n");
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}
