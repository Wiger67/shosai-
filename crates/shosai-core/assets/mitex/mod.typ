// 改寫自 MiTeX 0.2.4 packages/mitex/specs/mod.typ（Apache-2.0，見同資料夾 LICENSE）
#import "prelude.typ": *
#import "latex/standard.typ": package as latex-std

#let packages = (latex-std,)
#let mitex-scope = packages.map(pkg => pkg.scope).sum()
