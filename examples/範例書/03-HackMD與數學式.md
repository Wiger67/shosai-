# HackMD 與數學式

書齋的 Markdown 語法盡量與 [HackMD](https://hackmd.io) 一致，在 HackMD 寫好的筆記可以直接搬過來。

[TOC]

## 數學式

行內數學式用單個錢字號：質能等價 $E = mc^2$，圓面積 $A = \pi r^2$。

獨立的數學式用兩個錢字號：

$$
\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}
$$

也可以用 `math` 程式碼區塊：

```math
\begin{aligned}
\nabla \cdot \mathbf{E} &= \frac{\rho}{\varepsilon_0} \\
\nabla \times \mathbf{B} &= \mu_0 \mathbf{J} + \mu_0 \varepsilon_0 \frac{\partial \mathbf{E}}{\partial t}
\end{aligned}
```

矩陣與分段函數：$\begin{pmatrix} a & b \\ c & d \end{pmatrix}$，以及

$$
f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & \text{其他} \end{cases}
$$

## 文字標記

==螢光筆標記==、++插入（底線）++、~~刪除線~~、上標 x^2^、下標 H~2~O。
HackMD 預設「單一換行就換行」，就像這一行。
表情符號 :smile: :rocket: :books:

## 提示框

:::info
**資訊**：這是 `:::info` 提示框。
:::

:::success
完成！這是 `:::success`。
:::

:::warning 注意
可以在提示框後面加標題。
:::

:::danger
危險操作請三思。
:::

:::spoiler 點我看答案
答案是 $42$。
:::

## 程式碼行號

```rust=
fn main() {
    println!("加上 = 就會顯示行號");
}
```

```python=101
print("從第 101 行開始")
```

## 定義清單

Markdown
: 一種輕量標記語言。

Typst
: 新一代的排版系統，書齋用它來輸出 PDF。

## 圖片尺寸

![小一點的標誌](images/logo.svg =120x)
