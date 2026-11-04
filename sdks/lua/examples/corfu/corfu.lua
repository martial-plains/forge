-- corfu for Forge: completion at point in a popup, opening by itself
-- once typing pauses after a few characters (corfu-auto). C-n / C-p (or
-- the arrows) move, TAB or RET inserts, C-g closes, and anything else
-- keeps typing. M-TAB (or C-M-i) opens it by hand.
--
-- Candidates: words from the open buffers (built in), this plugin's
-- language keywords (like cape-keyword), and other plugins' sources (the
-- lsp plugin's, for one). Forge filters and draws; sources are asked
-- once per completion.

local SETTINGS = { popup = true, auto = true, delay = 200, min_prefix = 3, lines = 10 }

local KEYWORDS = {
  ["rust-mode"] = { "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "Option", "Result", "Some", "None", "Ok", "Err", "String", "Vec", "Box", "println" },
  ["python-mode"] = { "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
    "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
    "yield", "print", "len", "range", "enumerate", "isinstance" },
  ["js-mode"] = { "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
    "delete", "else", "export", "extends", "false", "finally", "for", "function", "if", "import", "in",
    "instanceof", "let", "new", "null", "return", "static", "super", "switch", "this", "throw", "true",
    "try", "typeof", "undefined", "var", "void", "while", "yield", "console" },
  ["c-mode"] = { "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
    "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register", "return", "short",
    "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile",
    "while", "bool", "size_t", "uint8_t", "uint32_t", "int64_t", "NULL", "printf", "malloc", "free" },
  ["go-mode"] = { "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
    "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select",
    "struct", "switch", "type", "var", "nil", "true", "false", "string", "error", "make", "append", "len" },
  ["lua-mode"] = { "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while", "pairs",
    "ipairs", "print", "require", "string", "table", "tostring", "tonumber" },
  ["sh-mode"] = { "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
    "local", "return", "select", "then", "until", "while", "echo", "export", "printf", "readonly" },
}

local modes = {}
for mode in pairs(KEYWORDS) do modes[#modes + 1] = mode end
table.sort(modes)

-- Byte index in `s` of character column `col` (0-based).
local function byte_at(s, col)
  return utf8.offset(s, col + 1) or #s + 1
end

forge.completion.source("keywords", function(buf, line, col, reply)
  local text = buf:line(line)
  local stop = byte_at(text, col)
  local start = stop
  while start > 1 and text:sub(start - 1, start - 1):match("[%w_]") do start = start - 1 end
  local prefix = text:sub(start, stop - 1)
  local items = {}
  for _, k in ipairs(KEYWORDS[buf:major_mode()] or {}) do
    if #prefix > 0 and k ~= prefix and k:sub(1, #prefix) == prefix then
      items[#items + 1] = { text = k, annotation = "keyword" }
    end
  end
  local start_col = utf8.len(text:sub(1, start - 1)) or 0
  reply(line, start_col, line, col, items)
end, { modes = modes })

local auto = true
forge.completion.set_at_point_ui(SETTINGS)

forge.command.create("corfu-auto-mode", function()
  auto = not auto
  SETTINGS.auto = auto
  forge.completion.set_at_point_ui(SETTINGS)
  forge.notify("Corfu auto completion " .. (auto and "on" or "off (M-TAB still completes)"))
end, { doc = "Turn opening the completion popup by itself on or off." })
