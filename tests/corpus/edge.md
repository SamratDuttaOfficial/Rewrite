---
title: Edge cases
tags: [test, markdown]
---

# Heading one

Setext heading
==============

Second setext
-------------

A paragraph with **bold**, *italic*, ***both***, ~~struck~~, `code`, and a [link](https://example.com "title").
An autolink <https://rust-lang.org> and a bare www.example.com address. Inline maths $e^{i\pi} + 1 = 0$ and
an escaped \*star\*. A footnote[^note] and a reference [link][ref].

[ref]: https://example.org/reference
[^note]: The footnote text.

> A quote that goes on
> for two lines.
>
> > A nested quote.

1. First
2. Second
   - Nested bullet
   - Another one
     1. Deep number
3. Third

- [ ] An open task
- [x] A done task

| Left | Centre | Right |
|:-----|:------:|------:|
| a    | b      | c     |
| a \| pipe | **bold** | `x` |

```python
def hello():
    return "world"
```

    indented code block
    second line

$$
\int_0^1 x^2 \, dx = \frac{1}{3}
$$

***

![A picture](images/missing.png)

Line one<br>line two &amp; an entity &copy;.

<div class="note">Some <em>HTML</em> inline.</div>

<!-- a comment that should be protected -->

Text with {#custom-id} attributes and a lazy
continuation line.
