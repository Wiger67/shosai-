//! LaTeX 數學式：以 [MiTeX](https://github.com/mitex-rs/mitex) 轉成 Typst，再交給 Typst 排版。
//!
//! PDF 直接嵌入轉換後的 Typst；GUI 預覽與 HTML 則把單一數學式編譯成 SVG。

use crate::export::typst::typst_str;

/// MiTeX 的 Typst 定義檔（虛擬路徑, 內容），編譯時放進 Typst 的虛擬檔案系統。
pub const MITEX_FILES: &[(&str, &str)] = &[
    ("/__mitex/mod.typ", include_str!("../assets/mitex/mod.typ")),
    ("/__mitex/prelude.typ", include_str!("../assets/mitex/prelude.typ")),
    ("/__mitex/latex/standard.typ", include_str!("../assets/mitex/latex/standard.typ")),
];

/// 文件開頭要加的匯入。
pub const MITEX_IMPORT: &str = "#import \"/__mitex/mod.typ\": mitex-scope\n";

/// LaTeX → Typst 數學語法。
pub fn latex_to_typst(tex: &str) -> Result<String, String> {
    mitex::convert_math(tex.trim(), None).map(|s| fix_symbols(&s))
}

/// MiTeX 0.2 的符號名稱是舊版 Typst 的；新版已移除或改名的，直接換成 Unicode 字元。
const RENAMED: &[(&str, &str)] = &[
    ("angle.l", "⟨"),
    ("angle.r", "⟩"),
    ("arrow.l.dash", "⇠"),
    ("arrow.r.dash", "⇢"),
    ("ast.circle", "⊛"),
    ("bracket.l.double", "⟦"),
    ("bracket.r.double", "⟧"),
    ("circle.nested", "⊚"),
    ("dash.circle", "⊝"),
    ("diff", "∂"),
    ("dot.circle", "⊙"),
    ("dot.circle.big", "⨀"),
    ("minus.circle", "⊖"),
    ("ohm.inv", "℧"),
    ("planck.reduce", "ℏ"),
    ("plus.circle", "⊕"),
    ("plus.circle.big", "⨁"),
    ("sect", "∩"),
    ("sect.big", "⋂"),
    ("sect.double", "⋒"),
    ("sect.sq", "⊓"),
    ("times.circle", "⊗"),
    ("times.circle.big", "⨂"),
];

fn fix_symbols(typst: &str) -> String {
    let mut out = String::with_capacity(typst.len());
    let mut rest = typst;
    let mut in_str = false;
    while let Some(c) = rest.chars().next() {
        if c == '"' {
            in_str = !in_str;
        }
        if !in_str && c.is_ascii_alphabetic() {
            let len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.')).unwrap_or(rest.len());
            let token = rest[..len].trim_end_matches('.');
            let prev_ok = !out.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == '#');
            match RENAMED.iter().find(|(old, _)| *old == token) {
                Some((_, new)) if prev_ok => out.push_str(new),
                _ => out.push_str(token),
            }
            rest = &rest[token.len()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// 產生一段 Typst 程式碼（以 `#` 開頭，可直接放在標記模式中）。
/// 轉換失敗時以等寬紅字顯示原始 LaTeX。
pub fn typst_equation(tex: &str, display: bool) -> String {
    match latex_to_typst(tex) {
        Ok(converted) => format!(
            "#math.equation(block: {display}, eval({}, mode: \"math\", scope: mitex-scope))",
            typst_str(&converted)
        ),
        Err(_) => fallback(tex, display),
    }
}

/// 無法排版時的替代顯示。
pub fn fallback(tex: &str, display: bool) -> String {
    let delim = if display { "$$" } else { "$" };
    format!("#text(fill: rgb(\"#c0392b\"), raw({}))", typst_str(&format!("{delim}{}{delim}", tex.trim())))
}

/// 檢查數學式能否成功排版（需要 `pdf` feature；否則一律視為可以）。
pub fn check(tex: &str, display: bool) -> bool {
    #[cfg(feature = "pdf")]
    {
        render_svg(tex, display, 11.0, "000000").is_ok()
    }
    #[cfg(not(feature = "pdf"))]
    {
        let _ = display;
        latex_to_typst(tex).is_ok()
    }
}

/// 把一個數學式渲染成 SVG。
///
/// - `size_pt`：字級（pt）
/// - `color`：文字顏色，六位十六進位，例如 `"2b2620"`
#[cfg(feature = "pdf")]
pub fn render_svg(tex: &str, display: bool, size_pt: f32, color: &str) -> Result<String, String> {
    let converted = latex_to_typst(tex)?;
    let source = format!(
        "{MITEX_IMPORT}#set page(width: auto, height: auto, margin: (x: 1pt, y: 2pt), fill: none)\n\
         #set text(size: {size_pt}pt, fill: rgb(\"#{color}\"), font: (\"New Computer Modern Math\", \"Noto Serif CJK TC\", \"Noto Sans CJK TC\", \"PingFang TC\", \"Microsoft JhengHei\"))\n\
         #math.equation(block: {display}, eval({}, mode: \"math\", scope: mitex-scope))\n",
        typst_str(&converted)
    );
    crate::export::pdf::compile_svg(&source).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_symbols() {
        assert_eq!(fix_symbols("frac(diff  f ,diff  x )"), "frac(∂  f ,∂  x )");
        assert_eq!(fix_symbols("a plus.circle b differ"), "a ⊕ b differ");
        assert_eq!(fix_symbols("lr(angle.l x angle.r)"), "lr(⟨ x ⟩)");
    }

    #[test]
    fn converts() {
        assert!(latex_to_typst(r"\frac{a}{b}").unwrap().contains("frac"));
        assert!(typst_equation(r"x^2", false).starts_with("#math.equation(block: false"));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn renders_common_formulas() {
        let cases = [
            r"E = mc^2",
            r"\frac{a}{b} + \sqrt{x^2+y^2}",
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\begin{aligned} f(x) &= x^2 \\ g(x) &= \sin x \end{aligned}",
            r"\alpha, \beta, \Gamma, \mathbb{R}, \mathbf{v}, \vec{a}, \hat{x}",
            r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
            r"\left( \frac{1}{2} \right) \cdot \leq \geq \neq \infty \pm",
            r"\text{速度} = \frac{\Delta s}{\Delta t}",
            r"\overline{AB} \underbrace{x+y}_{n} \binom{n}{k}",
            r"a \xrightarrow{f} b",
            r"\mathcal{L} \operatorname{sgn}(x) \mathrm{d}x",
            r"\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}",
            r"\frac{\partial \mathbf{E}}{\partial t}",
            r"\begin{aligned} a &= b \\ c &= d \end{aligned}",
            r"x \ge 0 \le 1",
        ];
        for c in cases {
            if let Err(e) = render_svg(c, true, 12.0, "000000") {
                panic!("{c}\n{:?}\n{e}", latex_to_typst(c));
            }
        }
        assert!(!check(r"\frac{", false) || latex_to_typst(r"\frac{").is_ok());
    }
}
