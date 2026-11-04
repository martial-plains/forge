#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate crates/editor/data/input-methods.txt.

    python3 scripts/gen-input-methods.py > crates/editor/data/input-methods.txt

The input methods are written for Forge: their keys are rules in this
file, and the accented letters come from Unicode's own composition data
(unicodedata, which is the Unicode Character Database). Nothing is copied
from another editor's tables.

Format: a line starting with U+0001 begins a method (tab-separated: name,
language, title, deterministic, maximum-shortest, simple, conversion,
update function or "-", node count, documentation, forget). The lines
after it are the nodes of its key trie, "id TAB kids TAB definition...",
where kids are "char U+001F id" joined by U+001E, and the definition is
the candidates (or U+0003 and the name of a function Forge calls).
"""
import sys
import unicodedata as ud

METHODS = []  # (header dict, rules {keys: [cands]} or funcs)


def esc(s):
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


def emit(name, language, title, doc, rules, *, update="-", conversion=0,
         simple=1, funcs=None, deterministic=1):
    funcs = funcs or {}
    nodes = [{"kids": {}, "def": None}]

    def walk(keys):
        n = 0
        for c in keys:
            kids = nodes[n]["kids"]
            if c not in kids:
                nodes.append({"kids": {}, "def": None})
                kids[c] = len(nodes) - 1
            n = kids[c]
        return n

    for keys, cands in rules.items():
        nodes[walk(keys)]["def"] = list(cands)
    for keys, fn in funcs.items():
        nodes[walk(keys)]["def"] = ("func", fn)
    out = ["\x01\t" + "\t".join([
        name, language, esc(title), str(deterministic), "0", str(simple), str(conversion),
        update, str(len(nodes)), esc(doc), "1",
    ])]
    for i, n in enumerate(nodes):
        kids = "\x1e".join(esc(c) + "\x1f" + str(k) for c, k in n["kids"].items())
        d = n["def"]
        if d is None:
            out.append(f"{i}\t{kids}")
            continue
        if isinstance(d, tuple):
            rest = "\x03\t" + d[1]
        else:
            rest = "\t".join(esc(c) for c in d)
            if len(d) > 1:
                rest = "\x02\t" + rest  # a list: a digit picks one
        out.append(f"{i}\t{kids}\t{rest}")
    METHODS.append((name, "\n".join(out)))


# ------------------------------------------------------------ Latin

ACCENTS = {  # key: combining mark
    "'": "\u0301", "`": "\u0300", '"': "\u0308", "^": "\u0302",
    "~": "\u0303", ",": "\u0327", ")": "\u0306", "v": "\u030c",
    "-": "\u0304", ".": "\u0307", ";": "\u0328", "o": "\u030a",
}
LETTERS = [chr(c) for c in list(range(65, 91)) + list(range(97, 123))]
SPACING = {"'": "´", "`": "`", '"': "¨", "^": "^", "~": "~", ",": "¸", "-": "¯"}
EXTRA = {  # letters Unicode doesn't decompose
    "ae": "æ", "AE": "Æ", "oe": "œ", "OE": "Œ", "ss": "ß", "dh": "ð",
    "DH": "Ð", "th": "þ", "TH": "Þ", "o/": "ø", "O/": "Ø", "l/": "ł",
    "L/": "Ł", "d/": "đ", "D/": "Đ",
}


def composed(base, mark):
    s = ud.normalize("NFC", base + mark)
    return s if len(s) == 1 and s != base else None


def latin(prefix):
    rules = {}
    for key, mark in ACCENTS.items():
        for b in LETTERS:
            c = composed(b, mark)
            if c:
                rules[(key + b) if prefix else (b + key)] = [c]
    for key in ACCENTS:
        if prefix:
            # Twice: the accent by itself (the spacing mark, or the key).
            rules[key + key] = [SPACING.get(key, key), key]
        else:
            for b in LETTERS:
                if (b + key) in rules:
                    rules[b + key + key] = [b + key]
    for k, v in EXTRA.items():
        rules[k] = [v]
    if not prefix:
        # The letter alone shows while an accent may still follow.
        for b in LETTERS:
            if any(k.startswith(b) and len(k) > 1 for k in rules):
                rules.setdefault(b, [b])
    return rules


emit("latin-prefix", "Latin", "L>",
     "Latin letters with accents: the accent first (' then e is é).",
     latin(True), deterministic=0)
emit("latin-postfix", "Latin", "L<",
     "Latin letters with accents: the accent after (e then ' is é).",
     latin(False), deterministic=0)

# ------------------------------------------------------------ Greek

GREEK = [("a", "alpha"), ("b", "beta"), ("g", "gamma"), ("d", "delta"),
         ("e", "epsilon"), ("z", "zeta"), ("h", "eta"), ("q", "theta"),
         ("i", "iota"), ("k", "kappa"), ("l", "lamda"), ("m", "mu"),
         ("n", "nu"), ("x", "xi"), ("o", "omicron"), ("p", "pi"),
         ("r", "rho"), ("s", "sigma"), ("t", "tau"), ("u", "upsilon"),
         ("f", "phi"), ("c", "chi"), ("y", "psi"), ("w", "omega")]
greek = {}
for key, name in GREEK:
    greek[key] = [ud.lookup("GREEK SMALL LETTER " + name.upper())]
    greek[key.upper()] = [ud.lookup("GREEK CAPITAL LETTER " + name.upper())]
greek["j"] = ["ς"]
emit("greek", "Greek", "Ω", "Greek letters by sound (a is α, q is θ, j is final ς).", greek)

# ------------------------------------------------------------ Cyrillic

CYR = {
    "a": "а", "b": "б", "v": "в", "g": "г", "d": "д", "e": "е", "zh": "ж",
    "z": "з", "i": "и", "j": "й", "k": "к", "l": "л", "m": "м", "n": "н",
    "o": "о", "p": "п", "r": "р", "s": "с", "t": "т", "u": "у", "f": "ф",
    "h": "х", "ts": "ц", "ch": "ч", "sh": "ш", "shch": "щ", "y": "ы",
    "eh": "э", "yu": "ю", "ya": "я", "yo": "ё", "'": "ь", '"': "ъ",
}
cyr = {}
for k, v in CYR.items():
    cyr[k] = [v]
    if k[0].isalpha():
        cyr[k[0].upper() + k[1:]] = [v.upper()]
        cyr[k.upper()] = [v.upper()]
emit("russian-translit", "Russian", "RU",
     "Russian letters by transliteration (sh is ш, shch is щ).", cyr)

# ------------------------------------------------------------ TeX

tex = {}
for key, name in GREEK:
    tex["\\" + name] = [ud.lookup("GREEK SMALL LETTER " + name.upper())]
    tex["\\" + name.capitalize()] = [ud.lookup("GREEK CAPITAL LETTER " + name.upper())]
tex["\\lambda"] = ["λ"]
tex["\\Lambda"] = ["Λ"]
SYMS = """to→ rightarrow→ leftarrow← uparrow↑ downarrow↓ leftrightarrow↔
Rightarrow⇒ Leftarrow⇐ Leftrightarrow⇔ mapsto↦ leq≤ geq≥ neq≠ approx≈
equiv≡ sim∼ simeq≃ cong≅ propto∝ ll≪ gg≫ pm± mp∓ times× div÷ cdot·
ast∗ circ∘ bullet• oplus⊕ otimes⊗ sum∑ prod∏ int∫ oint∮ partial∂
nabla∇ infty∞ forall∀ exists∃ nexists∄ in∈ notin∉ ni∋ subset⊂
supset⊃ subseteq⊆ supseteq⊇ cup∪ cap∩ emptyset∅ setminus∖ wedge∧
vee∨ neg¬ top⊤ bot⊥ vdash⊢ models⊨ sqrt√ angle∠ therefore∴ because∵
ldots… cdots⋯ vdots⋮ ddots⋱ dagger† ddagger‡ S§ P¶ copyright©
pounds£ yen¥ euro€ degree° prime′ hbar ℏ ell ℓ Re ℜ Im ℑ aleph ℵ
langle⟨ rangle⟩ lceil⌈ rceil⌉ lfloor⌊ rfloor⌋ checkmark✓ star⋆""".replace("\n", " ").split()
i = 0
while i < len(SYMS):
    w = SYMS[i]
    # "name+char" in one word, or "name char" as two
    name = "".join(ch for ch in w if ch.isascii() and ch.isalpha())
    sym = w[len(name):]
    if not sym:
        sym = SYMS[i + 1]
        i += 1
    tex["\\" + name] = [sym]
    i += 1
for key, mark in {"'": "\u0301", "`": "\u0300", '"': "\u0308", "^": "\u0302", "~": "\u0303"}.items():
    for b in LETTERS:
        c = composed(b, mark)
        if c:
            tex["\\" + key + b] = [c]
tex["\\"] = ["\\"]
tex["\\ss"] = ["ß"]
tex["\\ae"] = ["æ"]
tex["\\AE"] = ["Æ"]
tex["\\o"] = ["ø"]
tex["\\O"] = ["Ø"]
emit("TeX", "UTF-8", "\\", "TeX-like input: \\alpha is α, \\to is →, \\'e is é.", tex)

# ------------------------------------------------------------ Japanese

ROMAJI = {}
V = "aiueo"


def row(prefix, kana):
    for v, k in zip(V, kana):
        ROMAJI[prefix + v] = k


row("", "あいうえお")
row("k", "かきくけこ"); row("g", "がぎぐげご")
row("s", "さしすせそ"); row("z", "ざじずぜぞ")
row("t", "たちつてと"); row("d", "だぢづでど")
row("n", "なにぬねの"); row("h", "はひふへほ")
row("b", "ばびぶべぼ"); row("p", "ぱぴぷぺぽ")
row("m", "まみむめも"); row("r", "らりるれろ")
row("x", "ぁぃぅぇぉ"); row("l", "ぁぃぅぇぉ")
row("w", "わゐうゑを")
ROMAJI["wi"] = "うぃ"; ROMAJI["we"] = "うぇ"
ROMAJI.update({"ya": "や", "yu": "ゆ", "yo": "よ", "xya": "ゃ", "xyu": "ゅ", "xyo": "ょ",
               "lya": "ゃ", "lyu": "ゅ", "lyo": "ょ", "xtu": "っ", "ltu": "っ",
               "xtsu": "っ", "ltsu": "っ", "xwa": "ゎ", "lwa": "ゎ",
               "shi": "し", "chi": "ち", "tsu": "つ", "fu": "ふ", "ji": "じ",
               "nn": "ん", "n'": "ん", "vu": "ゔ", "-": "ー", ",": "、", ".": "。",
               "[": "「", "]": "」", "/": "・", "~": "〜"})
for p, b in {"k": "き", "g": "ぎ", "s": "し", "z": "じ", "t": "ち", "d": "ぢ", "n": "に",
             "h": "ひ", "b": "び", "p": "ぴ", "m": "み", "r": "り"}.items():
    for v, s in zip("auo", "ゃゅょ"):
        ROMAJI[p + "y" + v] = b + s
    ROMAJI[p + "ye"] = b + "ぇ"
for p, b in {"sh": "し", "ch": "ち", "j": "じ"}.items():
    for v, s in zip("auoe", "ゃゅょぇ"):
        ROMAJI[p + v] = b + s
for p, b in {"f": "ふ", "v": "ゔ", "ts": "つ"}.items():
    for v, s in zip("aieo", "ぁぃぇぉ"):
        ROMAJI[p + v] = b + s
ROMAJI.update({"tha": "てゃ", "thi": "てぃ", "thu": "てゅ", "the": "てぇ", "tho": "てょ",
               "dha": "でゃ", "dhi": "でぃ", "dhu": "でゅ", "dhe": "でぇ", "dho": "でょ",
               "twu": "とぅ", "dwu": "どぅ", "whi": "うぃ", "whe": "うぇ", "who": "うぉ",
               "wha": "うぁ"})
# Rows where one syllable has two spellings (si, ti, tu, hu, zi).
ROMAJI.update({"si": "し", "ti": "ち", "tu": "つ", "hu": "ふ", "zi": "じ", "di": "ぢ", "du": "づ"})
ROMAJI.update({"ja": "じゃ", "ju": "じゅ", "jo": "じょ", "je": "じぇ", "ji": "じ"})
ROMAJI.update({"cha": "ちゃ", "chu": "ちゅ", "cho": "ちょ", "che": "ちぇ"})
ROMAJI.update({"sha": "しゃ", "shu": "しゅ", "sho": "しょ", "she": "しぇ"})
ROMAJI = {k: v for k, v in ROMAJI.items() if v}


def kata(s):
    return "".join(chr(ord(c) + 0x60) if "\u3041" <= c <= "\u3096" else c for c in s)


def half(s):
    m = {}
    for h in range(0xFF61, 0xFFA0):
        n = ud.normalize("NFKC", chr(h))
        if len(n) == 1:
            m[n] = chr(h)
    m["\u3099"] = "\uff9e"; m["\u309a"] = "\uff9f"
    out = ""
    for c in kata(s):
        if c in m:
            out += m[c]
        else:
            d = ud.normalize("NFD", c)
            out += "".join(m.get(p, p) for p in d) if len(d) > 1 else c
    return out


SWITCH = {"qq": "quail-japanese-switch-package", "qz": "quail-japanese-switch-package",
          "qk": "quail-japanese-switch-package", "qh": "quail-japanese-switch-package"}
emit("japanese", "Japanese", "Aあ",
     "Japanese input method by Roman transliteration and Kana-Kanji conversion.",
     {k: [v] for k, v in ROMAJI.items()}, update="quail-japanese-update-translation",
     conversion=1, simple=0, funcs=SWITCH)
emit("japanese-hiragana", "Japanese", "あ",
     "Japanese hiragana input method by Roman transliteration.",
     {k: [v] for k, v in ROMAJI.items()}, update="quail-japanese-update-translation",
     simple=0, funcs=SWITCH)
emit("japanese-katakana", "Japanese", "ア",
     "Japanese katakana input method by Roman transliteration.",
     {k: [kata(v)] for k, v in ROMAJI.items()},
     update="quail-japanese-katakana-update-translation", simple=0, funcs=SWITCH)
emit("japanese-hankaku-kana", "Japanese", "ｱ",
     "Japanese hankaku katakana input method by Roman transliteration.",
     {k: [half(v)] for k, v in ROMAJI.items()},
     update="quail-japanese-hankaku-update-translation", simple=0, funcs=SWITCH)
zen = {chr(c): [chr(c + 0xFEE0)] for c in range(0x21, 0x7F)}
zen[" "] = ["\u3000"]
emit("japanese-zenkaku", "Japanese", "Ａ",
     "Japanese zenkaku alpha numeric character input method.", zen, simple=0)
emit("japanese-ascii", "Japanese", "Aa",
     "Temporary ASCII input mode used within the input method `japanese'.", {},
     simple=0, funcs={"qq": "quail-japanese-switch-package"})

print("# Forge's input methods, generated by scripts/gen-input-methods.py")
print("# (see that file for the format). Nothing here is copied from another editor.")
for name, text in sorted(METHODS, key=lambda m: m[0].encode()):
    print(text)
