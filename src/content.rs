//! The content-stream operator writer, and the number rules every content stream and every refusal is written by.
//!
//! A content stream is written by forme's own writer from doubles, never through pdf-writer's `Content`, whose only real is `f32`: one operator a line, its operands before it separated by one space, as pdf-lib's `PDFOperator` writes them, so a stream forme writes can be token for token the oracle's. A number is written as pdf-lib's `numberToString` writes it ([`js_number`]); a name is escaped as pdf-lib's `PDFName` escapes it; an array is pdf-lib's `[ a b ]`; a hex string, an inline property list and anything the oracle wrote by hand are the operands [`Operand::Hex`], [`Operand::Dict`] and [`Operand::Raw`].

use std::fmt::Write as _;

/// `String(n)` in JavaScript: the shortest decimal that reads back as `n`, written out between 1e-7 and 1e21 and with an exponent outside, `-0` as `0`.
pub fn js_string(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    // Rust's `{}` is the shortest round-trip decimal and never uses an exponent, which is JavaScript's form inside its range; `{:e}` has the same digits with the exponent, which says which range the number is in and is JavaScript's form outside it.
    let scientific = format!("{n:e}");
    let (mantissa, power) = scientific.split_once('e').expect("{:e} writes an exponent");
    let power: i32 = power.parse().expect("an integer exponent");
    if (-6..21).contains(&power) {
        return format!("{n}");
    }
    format!("{mantissa}e{}{}", if power < 0 { "-" } else { "+" }, power.abs())
}

/// pdf-lib's `numberToString`, which writes every number of its content streams: `String(n)`, and for an exponent form its own decimal expansion, ported with its arithmetic.
///
/// Below 1, an exponent `e-k` is expanded by multiplying by 10^(k−1) and writing `0.` and k−1 zeros before the product's digits; at 1e21 and above, by dividing by 10^(k−20) and writing that many zeros after it. Neither branch is taken by any number token of the committed corpus (`tests/data/number-tokens.txt`); both are ported so a list that reaches one writes what the oracle writes.
pub fn js_number(n: f64) -> String {
    let text = js_string(n);
    if n.abs() < 1.0 {
        if let Some(k) = text.split_once("e-").and_then(|(_, k)| k.parse::<i32>().ok()).filter(|&k| k != 0) {
            let product = n.abs() * pow10(k - 1);
            let digits = js_string(product);
            let tail = digits.get(2..).unwrap_or("");
            return format!("{}0.{}{}", if n < 0.0 { "-" } else { "" }, "0".repeat((k - 1) as usize), tail);
        }
    } else if let Some(k) = text.split_once('+').and_then(|(_, k)| k.parse::<i32>().ok()).filter(|&k| k > 20) {
        let quotient = n / pow10(k - 20);
        return format!("{}{}", js_string(quotient), "0".repeat((k - 20) as usize));
    }
    text
}

/// 10^k as `Math.pow(10, k)` gives it for the small whole k pdf-lib asks for: the double nearest the power, which parsing the decimal gives exactly.
fn pow10(k: i32) -> f64 {
    format!("1e{k}").parse().expect("a power of ten")
}

/// `Math.round(x)`: the nearest integer, a half rounded toward positive infinity, where Rust's `f64::round` rounds it away from zero.
pub fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `Math.round(n * 1000) / 1000`, the writer's thousandths.
pub fn round3(n: f64) -> f64 {
    js_round(n * 1000.0) / 1000.0
}

/// One operand of an operator.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand<'a> {
    /// A number, by [`js_number`].
    Num(f64),
    /// A name, without its slash, escaped as pdf-lib escapes it.
    Name(&'a str),
    /// A hex string of these bytes, in uppercase, as pdf-lib writes Caster's glyph strings.
    Hex(&'a [u8]),
    /// An array, as pdf-lib's `PDFArray` writes it: `[ a b ]`, `[ ]` when empty.
    Array(Vec<Operand<'a>>),
    /// An inline property list, as Caster writes it: `<</MCID 0>>`, `<</Type /Pagination /Subtype /Header>>`.
    Dict(Vec<(&'a str, Operand<'a>)>),
    /// Text the oracle wrote by hand as an operand, such as the dash array `[]`, written as is.
    Raw(&'a str),
}

/// A content stream being written.
#[derive(Clone, Debug, Default)]
pub struct Content {
    buf: String,
}

impl Content {
    /// An empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one operator: its operands, each followed by a space, then its name and a newline.
    pub fn op(&mut self, operands: &[Operand], operator: &str) -> &mut Self {
        for operand in operands {
            write_operand(&mut self.buf, operand);
            self.buf.push(' ');
        }
        self.buf.push_str(operator);
        self.buf.push('\n');
        self
    }

    /// The stream's bytes.
    pub fn finish(self) -> Vec<u8> {
        self.buf.into_bytes()
    }
}

fn write_operand(out: &mut String, operand: &Operand) {
    match operand {
        Operand::Num(n) => out.push_str(&js_number(*n)),
        Operand::Name(name) => write_name(out, name),
        Operand::Hex(bytes) => {
            out.push('<');
            for byte in *bytes {
                let _ = write!(out, "{byte:02X}");
            }
            out.push('>');
        }
        Operand::Array(items) => {
            out.push_str("[ ");
            for item in items {
                write_operand(out, item);
                out.push(' ');
            }
            out.push(']');
        }
        Operand::Dict(entries) => {
            out.push_str("<<");
            for (at, (key, value)) in entries.iter().enumerate() {
                if at > 0 {
                    out.push(' ');
                }
                write_name(out, key);
                out.push(' ');
                write_operand(out, value);
            }
            out.push_str(">>");
        }
        Operand::Raw(text) => out.push_str(text),
    }
}

/// A name as pdf-lib's `PDFName.of` writes it: each UTF-16 code unit from `!` to `~` that is no delimiter and no `#` as itself, every other as `#` and its code in uppercase hex, two digits at least.
fn write_name(out: &mut String, name: &str) {
    out.push('/');
    for unit in name.encode_utf16() {
        let regular = (0x21..=0x7E).contains(&unit) && !b"()<>[]{}/%#".contains(&(unit as u8));
        if regular {
            out.push(unit as u8 as char);
        } else {
            let _ = write!(out, "#{unit:02X}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected texts are pdf-lib 1.17.1's own `numberToString`, run in node on 2026-11-02, the two exponent branches' quirks among them.
    #[test]
    fn numbers_are_written_as_pdf_lib_writes_them() {
        for (n, text) in [(0.0, "0"), (-0.0, "0"), (12.0, "12"), (-1.5, "-1.5"), (0.1 + 0.2, "0.30000000000000004"), (340.157, "340.157"), (1e20, "100000000000000000000"), (1.5e-7, "0.00000015"), (2e21, "2000000000000000000000"), (-3.3e-9, "-0.0000000033"), (1.234e25, "12339999999999998000000000"), (1e-7, "0.00000009999999999999999")] {
            assert_eq!(js_number(n), text, "{n:e}");
        }
        assert_eq!(js_string(1.5e-7), "1.5e-7");
        assert_eq!(js_string(1e21), "1e+21");
        assert_eq!(js_string(1e-7), "1e-7");
        assert_eq!(js_string(1.2e-6), "0.0000012");
    }

    #[test]
    fn a_half_rounds_toward_positive_infinity() {
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-0.4), -0.0);
        assert_eq!(round3(1.0005), js_round(1.0005 * 1000.0) / 1000.0);
    }

    #[test]
    fn operators_are_written_one_a_line() {
        let mut content = Content::new();
        content.op(&[Operand::Name("Artifact"), Operand::Dict(vec![("Type", Operand::Name("Pagination")), ("Subtype", Operand::Name("Header"))])], "BDC");
        content.op(&[Operand::Name("H1"), Operand::Dict(vec![("MCID", Operand::Num(0.0))])], "BDC");
        content.op(&[Operand::Raw("[]"), Operand::Num(0.0)], "d");
        content.op(&[Operand::Array(vec![Operand::Hex(&[0x00, 0x17]), Operand::Num(19.0), Operand::Hex(&[0x00, 0x2f])])], "TJ");
        content.op(&[Operand::Name("a b#(")], "Tf");
        assert_eq!(String::from_utf8(content.finish()).unwrap(), "/Artifact <</Type /Pagination /Subtype /Header>> BDC\n/H1 <</MCID 0>> BDC\n[] 0 d\n[ <0017> 19 <002F> ] TJ\n/a#20b#23#28 Tf\n");
    }

    /// Every distinct number token of the oracle's content streams, dumped by Caster's `scripts/number-tokens.ts`: parsed as a double and formatted back, each must be the token the oracle wrote.
    #[test]
    fn every_number_token_of_the_oracle_comes_back_as_written() {
        let tokens: Vec<&str> = include_str!("../tests/data/number-tokens.txt").lines().collect();
        assert!(tokens.len() > 1000, "the dump reached the corpus ({} tokens)", tokens.len());
        let differ: Vec<String> = tokens.iter().filter_map(|&token| {
            let back = js_number(token.parse::<f64>().expect("a number token"));
            (back != token).then(|| format!("{token} -> {back}"))
        }).collect();
        assert!(differ.is_empty(), "{} of {} tokens come back otherwise: {}", differ.len(), tokens.len(), differ.join(", "));
    }
}
