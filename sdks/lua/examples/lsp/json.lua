-- A small JSON encoder/decoder for the LSP plugin (JSON-RPC messages).
--
-- json.null stands for null; json.array(t) marks a table as an array
-- (an empty table is otherwise an object). Decoded arrays are marked, and
-- null decodes to json.null, so values round-trip.

json = {}

json.null = setmetatable({}, { __tostring = function() return "null" end })
local ARRAY = {}

function json.array(t)
  return setmetatable(t or {}, ARRAY)
end

local function is_array(t)
  if getmetatable(t) == ARRAY then return true end
  local n = 0
  for k in pairs(t) do
    if type(k) ~= "number" then return false end
    n = n + 1
  end
  return n > 0 and n == #t
end

local ESC = { ['"'] = '\\"', ['\\'] = '\\\\', ['\b'] = '\\b', ['\f'] = '\\f', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t' }

local function encode_string(s)
  return '"' .. s:gsub('[%c"\\]', function(c)
    return ESC[c] or string.format("\\u%04x", c:byte())
  end) .. '"'
end

local encode

local function encode_table(t, out)
  if t == json.null then
    out[#out + 1] = "null"
  elseif is_array(t) then
    out[#out + 1] = "["
    for i, v in ipairs(t) do
      if i > 1 then out[#out + 1] = "," end
      encode(v, out)
    end
    out[#out + 1] = "]"
  else
    out[#out + 1] = "{"
    local first = true
    for k, v in pairs(t) do
      if not first then out[#out + 1] = "," end
      first = false
      out[#out + 1] = encode_string(tostring(k))
      out[#out + 1] = ":"
      encode(v, out)
    end
    out[#out + 1] = "}"
  end
end

encode = function(v, out)
  local ty = type(v)
  if ty == "nil" then
    out[#out + 1] = "null"
  elseif ty == "boolean" then
    out[#out + 1] = tostring(v)
  elseif ty == "number" then
    if math.type(v) == "integer" or v ~= v or v == math.huge or v == -math.huge then
      out[#out + 1] = math.type(v) == "integer" and tostring(v) or "null"
    else
      out[#out + 1] = string.format("%.14g", v)
    end
  elseif ty == "string" then
    out[#out + 1] = encode_string(v)
  elseif ty == "table" then
    encode_table(v, out)
  else
    error("can't encode a " .. ty .. " as JSON")
  end
end

function json.encode(v)
  local out = {}
  encode(v, out)
  return table.concat(out)
end

-- Decoding ---------------------------------------------------------------

local decode_value

local function skip(s, i)
  return s:find("[^ \t\r\n]", i) or #s + 1
end

local function decode_error(s, i, what)
  error(string.format("bad JSON at %d: %s (%q)", i, what, s:sub(i, i + 20)), 0)
end

local function utf8_of(cp)
  return utf8.char(cp)
end

local function decode_string(s, i)
  -- s:sub(i, i) == '"'
  local out = {}
  local j = i + 1
  while true do
    local a, b = s:find('["\\]', j)
    if not a then decode_error(s, i, "unterminated string") end
    out[#out + 1] = s:sub(j, a - 1)
    if s:sub(a, a) == '"' then
      return table.concat(out), a + 1
    end
    local c = s:sub(a + 1, a + 1)
    if c == "u" then
      local cp = tonumber(s:sub(a + 2, a + 5), 16)
      if not cp then decode_error(s, a, "bad \\u escape") end
      j = a + 6
      if cp >= 0xD800 and cp <= 0xDBFF and s:sub(j, j + 1) == "\\u" then
        local lo = tonumber(s:sub(j + 2, j + 5), 16)
        if lo and lo >= 0xDC00 and lo <= 0xDFFF then
          cp = 0x10000 + (cp - 0xD800) * 0x400 + (lo - 0xDC00)
          j = j + 6
        end
      end
      out[#out + 1] = utf8_of(cp)
    else
      local map = { b = "\b", f = "\f", n = "\n", r = "\r", t = "\t", ['"'] = '"', ["\\"] = "\\", ["/"] = "/" }
      if not map[c] then decode_error(s, a, "bad escape") end
      out[#out + 1] = map[c]
      j = a + 2
    end
  end
end

decode_value = function(s, i)
  i = skip(s, i)
  local c = s:sub(i, i)
  if c == "{" then
    local t = {}
    i = skip(s, i + 1)
    if s:sub(i, i) == "}" then return t, i + 1 end
    while true do
      if s:sub(i, i) ~= '"' then decode_error(s, i, "expected a key") end
      local k
      k, i = decode_string(s, i)
      i = skip(s, i)
      if s:sub(i, i) ~= ":" then decode_error(s, i, "expected :") end
      t[k], i = decode_value(s, i + 1)
      i = skip(s, i)
      local d = s:sub(i, i)
      if d == "}" then return t, i + 1 end
      if d ~= "," then decode_error(s, i, "expected , or }") end
      i = skip(s, i + 1)
    end
  elseif c == "[" then
    local t = json.array()
    i = skip(s, i + 1)
    if s:sub(i, i) == "]" then return t, i + 1 end
    while true do
      t[#t + 1], i = decode_value(s, i)
      i = skip(s, i)
      local d = s:sub(i, i)
      if d == "]" then return t, i + 1 end
      if d ~= "," then decode_error(s, i, "expected , or ]") end
      i = i + 1
    end
  elseif c == '"' then
    return decode_string(s, i)
  elseif s:sub(i, i + 3) == "true" then
    return true, i + 4
  elseif s:sub(i, i + 4) == "false" then
    return false, i + 5
  elseif s:sub(i, i + 3) == "null" then
    return json.null, i + 4
  else
    local num = s:match("^-?%d+%.?%d*[eE]?[-+]?%d*", i)
    if not num or num == "" then decode_error(s, i, "unexpected character") end
    return math.tointeger(tonumber(num)) or tonumber(num), i + #num
  end
end

function json.decode(s)
  local v, i = decode_value(s, 1)
  i = skip(s, i)
  if i <= #s then decode_error(s, i, "trailing characters") end
  return v
end
