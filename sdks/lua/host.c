/* The Lua plugin host: a Forge plugin component with Lua 5.4 inside.
 *
 * build.sh links this with Lua and the plugin's .lua files (embedded as
 * `forge_lua_scripts`, see there) into one component. At init it runs
 * the scripts in a fresh Lua state; they register commands and handlers
 * with the `forge` table below, which mirrors the init.rhai API
 * (docs/plugin-api.md) as far as the plugin world allows. The component
 * is sandboxed like any other plugin: no io or os library, and nothing
 * outside the forge:editor imports.
 *
 * Positions follow init.rhai: lines count from 1, columns from 0.
 */
#include "plugin.h"

#include "lauxlib.h"
#include "lua.h"
#include "lualib.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* From the generated scripts.c: name/source pairs, NULL-terminated. */
struct forge_lua_script { const char *name; const char *source; size_t len; };
extern const struct forge_lua_script forge_lua_scripts[];

static lua_State *L;

/* Registry tables: command name -> function, request id -> function,
 * event kind -> list of functions. */
static int commands_ref, answers_ref, handlers_ref, processes_ref, annotators_ref, groupers_ref, sources_ref;

/* ------------------------------------------------------------ helpers */

static plugin_string_t str(const char *s) {
    plugin_string_t r;
    plugin_string_set(&r, s);
    return r;
}

static plugin_string_t lstr(lua_State *L, int i) {
    size_t n;
    const char *s = luaL_checklstring(L, i, &n);
    plugin_string_t r = {(uint8_t *)s, n};
    return r;
}

static void push_str(lua_State *L, plugin_string_t *s) {
    lua_pushlstring(L, (const char *)s->ptr, s->len);
    plugin_string_free(s);
}

/* Raise Forge's error as a Lua error (frees it). */
static int raise(lua_State *L, plugin_string_t *err) {
    lua_pushlstring(L, (const char *)err->ptr, err->len);
    plugin_string_free(err);
    return lua_error(L);
}

static const char *opt_field(lua_State *L, int t, const char *key) {
    if (lua_type(L, t) != LUA_TTABLE) return NULL;
    lua_getfield(L, t, key);
    const char *s = lua_tostring(L, -1);
    lua_pop(L, 1);
    return s; /* still referenced by the table */
}

/* Keep a function in a registry table under an integer or string key. */
static void keep(int table, lua_State *L, int fn) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, table);
    lua_pushvalue(L, -2); /* key pushed by the caller */
    lua_pushvalue(L, fn);
    lua_rawset(L, -3);
    lua_pop(L, 2);
}

/* ------------------------------------------------------------ buffers */

#define BUFFER "forge.buffer"

typedef struct { uint32_t id; } lbuf;

static void push_buffer(lua_State *L, uint32_t id) {
    lbuf *b = lua_newuserdatauv(L, sizeof *b, 0);
    b->id = id;
    luaL_setmetatable(L, BUFFER);
}

static void push_own(lua_State *L, forge_editor_buffers_own_buffer_t own) {
    push_buffer(L, forge_editor_buffers_method_buffer_id(forge_editor_buffers_borrow_buffer(own)));
    forge_editor_buffers_buffer_drop_own(own);
}

/* The buffer argument as a handle; call drop() when done. Errors if the
 * buffer is gone. */
static forge_editor_buffers_own_buffer_t handle(lua_State *L) {
    lbuf *b = luaL_checkudata(L, 1, BUFFER);
    forge_editor_buffers_own_buffer_t own;
    if (!forge_editor_buffers_static_buffer_from_id(b->id, &own)) luaL_error(L, "buffer %d was killed", (int)b->id);
    if (!forge_editor_buffers_method_buffer_alive(forge_editor_buffers_borrow_buffer(own))) {
        forge_editor_buffers_buffer_drop_own(own);
        luaL_error(L, "buffer %d was killed", (int)b->id);
    }
    return own;
}

#define B forge_editor_buffers_borrow_buffer(own)
#define DROP forge_editor_buffers_buffer_drop_own(own)

static forge_editor_types_position_t lpos(lua_State *L, int line, int col) {
    lua_Integer l = luaL_checkinteger(L, line), c = luaL_optinteger(L, col, 0);
    forge_editor_types_position_t p = {(uint32_t)(l > 1 ? l - 1 : 0), (uint32_t)(c > 0 ? c : 0)};
    return p;
}

static int push_pos(lua_State *L, forge_editor_types_position_t p) {
    lua_pushinteger(L, (lua_Integer)p.line + 1);
    lua_pushinteger(L, p.column);
    return 2;
}

static int b_id(lua_State *L) { lua_pushinteger(L, ((lbuf *)luaL_checkudata(L, 1, BUFFER))->id); return 1; }

static int b_alive(lua_State *L) {
    lbuf *b = luaL_checkudata(L, 1, BUFFER);
    forge_editor_buffers_own_buffer_t own;
    bool alive = forge_editor_buffers_static_buffer_from_id(b->id, &own);
    if (alive) { alive = forge_editor_buffers_method_buffer_alive(B); DROP; }
    lua_pushboolean(L, alive);
    return 1;
}

static int b_name(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t s;
    forge_editor_buffers_method_buffer_name(B, &s);
    DROP;
    push_str(L, &s);
    return 1;
}

static int b_path(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t s;
    bool some = forge_editor_buffers_method_buffer_path(B, &s);
    DROP;
    if (some) push_str(L, &s); else lua_pushnil(L);
    return 1;
}

static int b_modified(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    lua_pushboolean(L, forge_editor_buffers_method_buffer_modified(B));
    DROP;
    return 1;
}

static int b_version(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    lua_pushinteger(L, (lua_Integer)forge_editor_buffers_method_buffer_version(B));
    DROP;
    return 1;
}

static int b_text(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t s;
    forge_editor_buffers_method_buffer_text(B, &s);
    DROP;
    push_str(L, &s);
    return 1;
}

static int b_line_count(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    lua_pushinteger(L, forge_editor_buffers_method_buffer_line_count(B));
    DROP;
    return 1;
}

static int b_line(lua_State *L) {
    lua_Integer n = luaL_checkinteger(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t s;
    forge_editor_buffers_method_buffer_line(B, (uint32_t)(n > 1 ? n - 1 : 0), &s);
    DROP;
    if (n < 1) { plugin_string_free(&s); lua_pushliteral(L, ""); } else push_str(L, &s);
    return 1;
}

static int b_point(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_position_t p;
    forge_editor_buffers_method_buffer_point(B, &p);
    DROP;
    return push_pos(L, p);
}

static int b_set_point(lua_State *L) {
    forge_editor_types_position_t p = lpos(L, 2, 3);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_set_point(B, &p);
    DROP;
    return 0;
}

static int b_insert(lua_State *L) {
    plugin_string_t t = lstr(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_insert(B, &t);
    DROP;
    return 0;
}

/* set_mark(line, col) sets (and activates) the mark; set_mark() clears it. */
static int b_set_mark(lua_State *L) {
    forge_editor_buffers_own_buffer_t own;
    if (lua_isnoneornil(L, 2)) {
        own = handle(L);
        forge_editor_buffers_method_buffer_set_mark(B, NULL);
    } else {
        forge_editor_types_position_t p = lpos(L, 2, 3);
        own = handle(L);
        forge_editor_buffers_method_buffer_set_mark(B, &p);
    }
    DROP;
    return 0;
}

static int b_mark(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_position_t p;
    bool some = forge_editor_buffers_method_buffer_mark(B, &p);
    DROP;
    return some ? push_pos(L, p) : 0;
}

static int b_region(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_span_t s;
    bool some = forge_editor_buffers_method_buffer_region(B, &s);
    DROP;
    if (!some) return 0;
    push_pos(L, s.start);
    return 2 + push_pos(L, s.end);
}

static int b_region_text(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_span_t s;
    if (!forge_editor_buffers_method_buffer_region(B, &s)) { DROP; lua_pushnil(L); return 1; }
    plugin_string_t t;
    forge_editor_buffers_method_buffer_text_in(B, &s, &t);
    DROP;
    push_str(L, &t);
    return 1;
}

static forge_editor_types_span_t lspan(lua_State *L, int i) {
    forge_editor_types_span_t s = {lpos(L, i, i + 1), lpos(L, i + 2, i + 3)};
    return s;
}

/* replace(l1, c1, l2, c2, text) */
static int b_replace(lua_State *L) {
    forge_editor_types_span_t s = lspan(L, 2);
    plugin_string_t t = lstr(L, 6);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_replace(B, &s, &t);
    DROP;
    return 0;
}

/* text_in(l1, c1, l2, c2) */
static int b_text_in(lua_State *L) {
    forge_editor_types_span_t s = lspan(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t t;
    forge_editor_buffers_method_buffer_text_in(B, &s, &t);
    DROP;
    push_str(L, &t);
    return 1;
}

/* search_forward(text) / search_backward(text): move point past / to the
 * match; true if found. */
static int search(lua_State *L, bool forward) {
    plugin_string_t q = lstr(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_position_t p;
    forge_editor_types_span_t m;
    forge_editor_buffers_method_buffer_point(B, &p);
    bool found = forward ? forge_editor_buffers_method_buffer_search_forward(B, &q, &p, &m)
                         : forge_editor_buffers_method_buffer_search_backward(B, &q, &p, &m);
    if (found) forge_editor_buffers_method_buffer_set_point(B, forward ? &m.end : &m.start);
    DROP;
    lua_pushboolean(L, found);
    return 1;
}
static int b_search_forward(lua_State *L) { return search(L, true); }
static int b_search_backward(lua_State *L) { return search(L, false); }

/* search_regexp(pattern [, line, col]) -> l1, c1, l2, c2 or nil */
static int b_search_regexp(lua_State *L) {
    plugin_string_t q = lstr(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_types_position_t p;
    if (lua_isnoneornil(L, 3)) forge_editor_buffers_method_buffer_point(B, &p); else p = lpos(L, 3, 4);
    forge_editor_buffers_option_span_t m;
    plugin_string_t err;
    bool ok = forge_editor_buffers_method_buffer_search_regexp(B, &q, &p, &m, &err);
    DROP;
    if (!ok) return raise(L, &err);
    if (!m.is_some) return 0;
    push_pos(L, m.val.start);
    return 2 + push_pos(L, m.val.end);
}

static int b_major_mode(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_string_t s;
    forge_editor_buffers_method_buffer_major_mode(B, &s);
    DROP;
    push_str(L, &s);
    return 1;
}

static int b_set_major_mode(lua_State *L) {
    plugin_string_t m = lstr(L, 2), err;
    forge_editor_buffers_own_buffer_t own = handle(L);
    bool ok = forge_editor_buffers_method_buffer_set_major_mode(B, &m, &err);
    DROP;
    return ok ? 0 : raise(L, &err);
}

static int b_minor_modes(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    plugin_list_string_t l;
    forge_editor_buffers_method_buffer_minor_modes(B, &l);
    DROP;
    lua_createtable(L, (int)l.len, 0);
    for (size_t i = 0; i < l.len; i++) {
        lua_pushlstring(L, (const char *)l.ptr[i].ptr, l.ptr[i].len);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_string_free(&l);
    return 1;
}

static int b_enable_minor_mode(lua_State *L) {
    plugin_string_t m = lstr(L, 2), err;
    forge_editor_buffers_own_buffer_t own = handle(L);
    bool ok = forge_editor_buffers_method_buffer_enable_minor_mode(B, &m, &err);
    DROP;
    return ok ? 0 : raise(L, &err);
}

static int b_disable_minor_mode(lua_State *L) {
    plugin_string_t m = lstr(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_disable_minor_mode(B, &m);
    DROP;
    return 0;
}

static int b_set_option(lua_State *L) {
    plugin_string_t n = lstr(L, 2), v, err;
    if (lua_isnoneornil(L, 3)) v = str("");
    else if (lua_isboolean(L, 3)) v = str(lua_toboolean(L, 3) ? "true" : "false");
    else v = lstr(L, 3); /* numbers convert */
    forge_editor_buffers_own_buffer_t own = handle(L);
    bool ok = forge_editor_buffers_method_buffer_set_option(B, &n, &v, &err);
    DROP;
    return ok ? 0 : raise(L, &err);
}

static int b_option(lua_State *L) {
    plugin_string_t n = lstr(L, 2), v;
    forge_editor_buffers_own_buffer_t own = handle(L);
    bool some = forge_editor_buffers_method_buffer_get_option(B, &n, &v);
    DROP;
    if (some) push_str(L, &v); else lua_pushnil(L);
    return 1;
}

static int b_indent_line(lua_State *L) {
    lua_Integer n = luaL_checkinteger(L, 2);
    forge_editor_buffers_own_buffer_t own = handle(L);
    lua_pushboolean(L, forge_editor_buffers_method_buffer_indent_line(B, (uint32_t)(n > 1 ? n - 1 : 0)));
    DROP;
    return 1;
}

static int b_show(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_show(B);
    DROP;
    return 0;
}

static int b_kill(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_buffers_method_buffer_kill(B);
    DROP;
    return 0;
}

/* set_decorations({ {l1, c1, l2, c2, face, message}, ... }) */
static int b_set_decorations(lua_State *L) {
    luaL_checktype(L, 2, LUA_TTABLE);
    size_t n = lua_rawlen(L, 2);
    forge_editor_decorations_decoration_t *d = calloc(n ? n : 1, sizeof *d);
    for (size_t i = 0; i < n; i++) {
        lua_rawgeti(L, 2, (lua_Integer)i + 1);
        int t = lua_gettop(L);
        luaL_checktype(L, t, LUA_TTABLE);
        for (int k = 1; k <= 6; k++) lua_rawgeti(L, t, k);
        d[i].span = lspan(L, t + 1);
        d[i].face = lstr(L, t + 5);
        if (lua_isstring(L, t + 6)) { d[i].message.is_some = true; d[i].message.val = lstr(L, t + 6); }
        lua_settop(L, t - 1); /* the strings stay alive in the table */
    }
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_decorations_list_decoration_t list = {d, n};
    plugin_string_t err;
    bool ok = forge_editor_decorations_set(B, &list, &err);
    DROP;
    free(d);
    return ok ? 0 : raise(L, &err);
}

static int b_clear_decorations(lua_State *L) {
    forge_editor_buffers_own_buffer_t own = handle(L);
    forge_editor_decorations_clear(B);
    DROP;
    return 0;
}

static int b_eq(lua_State *L) {
    lbuf *a = luaL_checkudata(L, 1, BUFFER), *b = luaL_checkudata(L, 2, BUFFER);
    lua_pushboolean(L, a->id == b->id);
    return 1;
}

static int b_tostring(lua_State *L) {
    lua_pushfstring(L, "buffer %d", (int)((lbuf *)luaL_checkudata(L, 1, BUFFER))->id);
    return 1;
}

static const luaL_Reg buffer_methods[] = {
    {"id", b_id}, {"alive", b_alive}, {"name", b_name}, {"path", b_path}, {"modified", b_modified},
    {"version", b_version}, {"text", b_text}, {"text_in", b_text_in}, {"line_count", b_line_count},
    {"line", b_line}, {"point", b_point}, {"set_point", b_set_point}, {"insert", b_insert},
    {"region", b_region}, {"set_mark", b_set_mark}, {"mark", b_mark}, {"region_text", b_region_text}, {"replace", b_replace},
    {"search_forward", b_search_forward}, {"search_backward", b_search_backward},
    {"search_regexp", b_search_regexp}, {"major_mode", b_major_mode}, {"set_major_mode", b_set_major_mode},
    {"minor_modes", b_minor_modes}, {"enable_minor_mode", b_enable_minor_mode},
    {"disable_minor_mode", b_disable_minor_mode}, {"set_option", b_set_option}, {"option", b_option},
    {"indent_line", b_indent_line}, {"show", b_show}, {"kill", b_kill},
    {"set_decorations", b_set_decorations}, {"clear_decorations", b_clear_decorations}, {NULL, NULL}};

static int buf_current(lua_State *L) { push_own(L, forge_editor_buffers_static_buffer_current()); return 1; }

static int buf_all(lua_State *L) {
    forge_editor_buffers_list_own_buffer_t l;
    forge_editor_buffers_static_buffer_all(&l);
    lua_createtable(L, (int)l.len, 0);
    for (size_t i = 0; i < l.len; i++) {
        push_own(L, l.ptr[i]);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    free(l.ptr);
    return 1;
}

static int buf_find(lua_State *L) {
    plugin_string_t n = lstr(L, 1);
    forge_editor_buffers_own_buffer_t own;
    if (forge_editor_buffers_static_buffer_find(&n, &own)) push_own(L, own); else lua_pushnil(L);
    return 1;
}

static int buf_from_id(lua_State *L) {
    forge_editor_buffers_own_buffer_t own;
    if (forge_editor_buffers_static_buffer_from_id((uint32_t)luaL_checkinteger(L, 1), &own)) push_own(L, own);
    else lua_pushnil(L);
    return 1;
}

/* buffer.visit(path) -> buffer, or nil and an error */
static int buf_visit(lua_State *L) {
    plugin_string_t p = lstr(L, 1), err;
    forge_editor_buffers_own_buffer_t own;
    if (forge_editor_buffers_static_buffer_visit(&p, &own, &err)) {
        push_own(L, own);
        return 1;
    }
    lua_pushnil(L);
    push_str(L, &err);
    return 2;
}

static int buf_create(lua_State *L) {
    plugin_string_t n = lstr(L, 1);
    push_own(L, forge_editor_buffers_static_buffer_create(&n));
    return 1;
}

/* ----------------------------------------------------------- commands */

/* command.create(name, fn [, { doc = "..." }]) */
static int cmd_create(lua_State *L) {
    luaL_checkstring(L, 1);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    const char *doc = opt_field(L, 3, "doc");
    bool background = false;
    if (lua_type(L, 3) == LUA_TTABLE) {
        lua_getfield(L, 3, "background");
        background = lua_toboolean(L, -1);
        lua_pop(L, 1);
    }
    plugin_string_t n = lstr(L, 1), d = str(doc ? doc : ""), err;
    bool ok = background ? forge_editor_commands_register_background(&n, &d, &err)
                         : forge_editor_commands_register(&n, &d, &err);
    if (!ok) return raise(L, &err);
    lua_pushvalue(L, 1);
    keep(commands_ref, L, 2);
    return 0;
}

static int cmd_run(lua_State *L) {
    plugin_string_t n = lstr(L, 1), err;
    return forge_editor_commands_run(&n, &err) ? 0 : raise(L, &err);
}

static int cmd_exists(lua_State *L) {
    plugin_string_t n = lstr(L, 1);
    lua_pushboolean(L, forge_editor_commands_exists(&n));
    return 1;
}

/* ------------------------------------------------------------- keymap */

/* The scope from an options table: { mode = "..." } or { buffer = id }. */
static forge_editor_keymap_scope_t scope(lua_State *L, int opts) {
    forge_editor_keymap_scope_t s = {.tag = FORGE_EDITOR_KEYMAP_SCOPE_GLOBAL};
    const char *mode = opt_field(L, opts, "mode");
    if (mode) {
        s.tag = FORGE_EDITOR_KEYMAP_SCOPE_MODE;
        s.val.mode = str(mode);
    } else if (lua_type(L, opts) == LUA_TTABLE) {
        lua_getfield(L, opts, "buffer");
        if (lua_isinteger(L, -1)) {
            s.tag = FORGE_EDITOR_KEYMAP_SCOPE_BUFFER;
            s.val.buffer = (uint32_t)lua_tointeger(L, -1);
        } else if (luaL_testudata(L, -1, BUFFER)) {
            s.tag = FORGE_EDITOR_KEYMAP_SCOPE_BUFFER;
            s.val.buffer = ((lbuf *)lua_touserdata(L, -1))->id;
        }
        lua_pop(L, 1);
    }
    return s;
}

/* keymap.set(keys, command [, { mode = ... }]) */
static int km_set(lua_State *L) {
    if (lua_isfunction(L, 2)) return luaL_error(L, "keymap.set: in a plugin, bind a command name (make one with forge.command.create)");
    forge_editor_keymap_scope_t s = scope(L, 3);
    plugin_string_t k = lstr(L, 1), c = lstr(L, 2), err;
    return forge_editor_keymap_bind(&s, &k, &c, &err) ? 0 : raise(L, &err);
}

static int km_del(lua_State *L) {
    forge_editor_keymap_scope_t s = scope(L, 2);
    plugin_string_t k = lstr(L, 1), err;
    bool had;
    if (!forge_editor_keymap_unbind(&s, &k, &had, &err)) return raise(L, &err);
    lua_pushboolean(L, had);
    return 1;
}

static int km_remap(lua_State *L) {
    forge_editor_keymap_scope_t s = scope(L, 3);
    plugin_string_t a = lstr(L, 1), b = lstr(L, 2);
    forge_editor_keymap_remap(&s, &a, &b);
    return 0;
}

static int km_get(lua_State *L) {
    plugin_string_t k = lstr(L, 1), c;
    if (forge_editor_keymap_lookup(&k, &c)) push_str(L, &c); else lua_pushnil(L);
    return 1;
}

static int km_is_prefix(lua_State *L) {
    plugin_string_t k = lstr(L, 1);
    lua_pushboolean(L, forge_editor_keymap_is_prefix(&k));
    return 1;
}

/* keymap.execute(keys): type them, once this call returns. */
static int km_execute(lua_State *L) {
    plugin_string_t k = lstr(L, 1), err;
    return forge_editor_keymap_execute_keys(&k, &err) ? 0 : raise(L, &err);
}

static int km_where_is(lua_State *L) {
    plugin_string_t c = lstr(L, 1);
    plugin_list_string_t l;
    forge_editor_keymap_where_is(&c, &l);
    lua_createtable(L, (int)l.len, 0);
    for (size_t i = 0; i < l.len; i++) {
        lua_pushlstring(L, (const char *)l.ptr[i].ptr, l.ptr[i].len);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_string_free(&l);
    return 1;
}

static int cmd_doc(lua_State *L) {
    plugin_string_t n = lstr(L, 1), d;
    if (forge_editor_commands_doc(&n, &d)) push_str(L, &d); else lua_pushnil(L);
    return 1;
}

/* ------------------------------------------------------------ ui, log */

static void keep_answer(lua_State *L, uint32_t request, int fn) {
    lua_pushinteger(L, request);
    keep(answers_ref, L, fn);
}

/* ui.input({ prompt =, default = }, fn(answer or nil)) */
static int ui_input(lua_State *L) {
    luaL_checktype(L, 2, LUA_TFUNCTION);
    const char *prompt = opt_field(L, 1, "prompt"), *def = opt_field(L, 1, "default");
    plugin_string_t p = str(prompt ? prompt : "> "), i = str(def ? def : "");
    keep_answer(L, forge_editor_minibuffer_read_string(&p, &i), 2);
    return 0;
}

/* ui.select(items, { prompt =, require_match = }, fn(choice or nil)) */
static int ui_select(lua_State *L) {
    luaL_checktype(L, 1, LUA_TTABLE);
    luaL_checktype(L, 3, LUA_TFUNCTION);
    const char *prompt = opt_field(L, 2, "prompt");
    bool require = true;
    if (lua_type(L, 2) == LUA_TTABLE) {
        lua_getfield(L, 2, "require_match");
        if (!lua_isnil(L, -1)) require = lua_toboolean(L, -1);
        lua_pop(L, 1);
    }
    size_t n = lua_rawlen(L, 1);
    plugin_string_t *items = calloc(n ? n : 1, sizeof *items);
    for (size_t i = 0; i < n; i++) {
        lua_rawgeti(L, 1, (lua_Integer)i + 1);
        size_t len;
        const char *s = lua_tolstring(L, -1, &len);
        if (!s) { free(items); return luaL_error(L, "ui.select: item %d isn't a string", (int)i + 1); }
        items[i].ptr = (uint8_t *)s;
        items[i].len = len;
        lua_pop(L, 1); /* the table keeps it alive */
    }
    plugin_string_t p = str(prompt ? prompt : "Select: ");
    plugin_list_string_t list = {items, n};
    uint32_t r = forge_editor_minibuffer_completing_read(&p, &list, require);
    free(items);
    keep_answer(L, r, 3);
    return 0;
}

/* ui.read_key(prompt, fn(key or nil)) */
static int ui_read_key(lua_State *L) {
    luaL_checktype(L, 2, LUA_TFUNCTION);
    plugin_string_t p = lstr(L, 1);
    lua_pushinteger(L, forge_editor_minibuffer_read_key(&p));
    keep(answers_ref, L, 2);
    return 0;
}

/* ui.confirm(prompt, fn(yes)) */
static int ui_confirm(lua_State *L) {
    luaL_checktype(L, 2, LUA_TFUNCTION);
    plugin_string_t p = lstr(L, 1);
    uint32_t r = forge_editor_minibuffer_y_or_n(&p);
    /* Kept as { confirm = fn } so on_answer passes a boolean. */
    lua_newtable(L);
    lua_pushvalue(L, 2);
    lua_setfield(L, -2, "confirm");
    lua_pushinteger(L, r);
    keep(answers_ref, L, lua_gettop(L) - 1);
    lua_pop(L, 1);
    return 0;
}

static forge_editor_log_level_t level(const char *s) {
    if (!s) return FORGE_EDITOR_LOG_LEVEL_INFO;
    if (!strcmp(s, "debug")) return FORGE_EDITOR_LOG_LEVEL_DEBUG;
    if (!strcmp(s, "warn")) return FORGE_EDITOR_LOG_LEVEL_WARN;
    if (!strcmp(s, "error")) return FORGE_EDITOR_LOG_LEVEL_ERROR;
    return FORGE_EDITOR_LOG_LEVEL_INFO;
}

/* notify(message [, level]): the echo area for info, *Messages* too. */
static int f_notify(lua_State *L) {
    plugin_string_t m = lstr(L, 1);
    const char *lv = luaL_optstring(L, 2, "info");
    if (!strcmp(lv, "info")) forge_editor_minibuffer_message(&m);
    else forge_editor_log_log(level(lv), &m);
    return 0;
}

/* log(level, message) */
static int f_log(lua_State *L) {
    const char *lv = luaL_checkstring(L, 1);
    plugin_string_t m = lstr(L, 2);
    forge_editor_log_log(level(lv), &m);
    return 0;
}

/* print(...) goes to *Messages* at info level. */
static int f_print(lua_State *L) {
    int n = lua_gettop(L);
    luaL_Buffer b;
    luaL_buffinit(L, &b);
    for (int i = 1; i <= n; i++) {
        if (i > 1) luaL_addchar(&b, '\t');
        luaL_tolstring(L, i, NULL);
        luaL_addvalue(&b);
    }
    luaL_pushresult(&b);
    plugin_string_t m = lstr(L, -1);
    forge_editor_log_log(FORGE_EDITOR_LOG_LEVEL_INFO, &m);
    return 0;
}

/* -------------------------------------------------------- modes, events */

static void push_strings(lua_State *L, plugin_list_string_t *l) {
    lua_createtable(L, (int)l->len, 0);
    for (size_t i = 0; i < l->len; i++) {
        lua_pushlstring(L, (const char *)l->ptr[i].ptr, l->ptr[i].len);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_string_free(l);
}

/* mode.define_major(name, { parent =, extensions = { ... } }) */
static int mode_define_major(lua_State *L) {
    plugin_string_t n = lstr(L, 1), parent, err;
    const char *p = opt_field(L, 2, "parent");
    if (p) parent = str(p);
    size_t count = 0;
    plugin_string_t *exts = NULL;
    if (lua_type(L, 2) == LUA_TTABLE) {
        lua_getfield(L, 2, "extensions");
        if (lua_istable(L, -1)) {
            count = lua_rawlen(L, -1);
            exts = calloc(count ? count : 1, sizeof *exts);
            for (size_t i = 0; i < count; i++) {
                lua_rawgeti(L, -1, (lua_Integer)i + 1);
                size_t len;
                const char *s = lua_tolstring(L, -1, &len);
                exts[i].ptr = (uint8_t *)(s ? s : "");
                exts[i].len = s ? len : 0;
                lua_pop(L, 1);
            }
        }
        lua_pop(L, 1);
    }
    plugin_list_string_t list = {exts, count};
    bool ok = forge_editor_modes_define_major_mode(&n, p ? &parent : NULL, &list, &err);
    free(exts);
    return ok ? 0 : raise(L, &err);
}

static int mode_define_minor(lua_State *L) {
    plugin_string_t n = lstr(L, 1), err;
    return forge_editor_modes_define_minor_mode(&n, &err) ? 0 : raise(L, &err);
}

static int mode_major_modes(lua_State *L) {
    plugin_list_string_t l;
    forge_editor_modes_major_modes(&l);
    push_strings(L, &l);
    return 1;
}

static int mode_minor_modes(lua_State *L) {
    plugin_list_string_t l;
    forge_editor_modes_minor_modes(&l);
    push_strings(L, &l);
    return 1;
}

static const char *const EVENTS[] = {"buffer-opened", "buffer-killed", "buffer-changed", "buffer-saved",
                                     "mode-changed", NULL};

/* on(event, fn(buffer) [, { background = true }]): with background, the
 * event's handlers run in the background (see commands.register-background). */
static int f_on(lua_State *L) {
    int kind = luaL_checkoption(L, 1, NULL, EVENTS);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    bool background = false;
    if (lua_type(L, 3) == LUA_TTABLE) {
        lua_getfield(L, 3, "background");
        background = lua_toboolean(L, -1);
        lua_pop(L, 1);
    }
    if (background) forge_editor_events_subscribe_background((forge_editor_events_kind_t)kind);
    lua_rawgeti(L, LUA_REGISTRYINDEX, handlers_ref);
    if (lua_rawgeti(L, -1, kind) == LUA_TNIL) {
        lua_pop(L, 1);
        lua_newtable(L);
        lua_pushvalue(L, -1);
        lua_rawseti(L, -3, kind);
        if (!background) forge_editor_events_subscribe((forge_editor_events_kind_t)kind);
    }
    lua_pushvalue(L, 2);
    lua_rawseti(L, -2, (lua_Integer)lua_rawlen(L, -2) + 1);
    lua_pop(L, 2);
    return 0;
}

static int mode_set_lighter(lua_State *L) {
    plugin_string_t m = lstr(L, 1), l = lstr(L, 2), err;
    return forge_editor_modes_set_lighter(&m, &l, &err) ? 0 : raise(L, &err);
}

/* --------------------------------------------------------- completion */

static lua_Integer int_field(lua_State *L, int t, const char *key, lua_Integer def) {
    if (lua_type(L, t) != LUA_TTABLE) return def;
    lua_getfield(L, t, key);
    lua_Integer v = lua_isinteger(L, -1) ? lua_tointeger(L, -1) : def;
    lua_pop(L, 1);
    return v;
}

static bool bool_field(lua_State *L, int t, const char *key, bool def) {
    if (lua_type(L, t) != LUA_TTABLE) return def;
    lua_getfield(L, t, key);
    bool v = lua_isnil(L, -1) ? def : lua_toboolean(L, -1);
    lua_pop(L, 1);
    return v;
}

/* A Lua list of strings at `t` as a borrowed list (free .ptr). */
static plugin_list_string_t string_list(lua_State *L, int t) {
    plugin_list_string_t l = {NULL, 0};
    if (lua_type(L, t) != LUA_TTABLE) return l;
    l.len = lua_rawlen(L, t);
    l.ptr = calloc(l.len ? l.len : 1, sizeof *l.ptr);
    for (size_t i = 0; i < l.len; i++) {
        lua_rawgeti(L, t, (lua_Integer)i + 1);
        size_t len;
        const char *s = lua_tolstring(L, -1, &len);
        l.ptr[i].ptr = (uint8_t *)(s ? s : "");
        l.ptr[i].len = s ? len : 0;
        lua_pop(L, 1); /* the table keeps it alive */
    }
    return l;
}

/* The items of a menu call: a list of { label =, command = } tables, or
 * plain strings. A missing command means a separator. */
static forge_editor_menus_list_item_t menu_items(lua_State *L, int t) {
    luaL_checktype(L, t, LUA_TTABLE);
    size_t n = lua_rawlen(L, t);
    forge_editor_menus_list_item_t l = {NULL, n};
    if (n == 0) return l;
    l.ptr = calloc(n, sizeof(forge_editor_menus_item_t));
    for (size_t i = 0; i < n; i++) {
        lua_rawgeti(L, t, (lua_Integer)i + 1);
        if (lua_type(L, -1) == LUA_TSTRING) {
            l.ptr[i].label = lstr(L, -1);
            l.ptr[i].command = lstr(L, -1);
        } else {
            lua_getfield(L, -1, "label");
            l.ptr[i].label = lstr(L, -1);
            lua_pop(L, 1);
            lua_getfield(L, -1, "command");
            l.ptr[i].command = lstr(L, -1);
            lua_pop(L, 1);
        }
        lua_pop(L, 1);
    }
    return l;
}

static void menu_items_free(forge_editor_menus_list_item_t *l) {
    for (size_t i = 0; i < l->len; i++) {
        plugin_string_free(&l->ptr[i].label);
        plugin_string_free(&l->ptr[i].command);
    }
    free(l->ptr);
}

/* menus.define(title, items) */
static int menus_define(lua_State *L) {
    luaL_checkstring(L, 1);
    forge_editor_menus_list_item_t items = menu_items(L, 2);
    plugin_string_t title = lstr(L, 1), err;
    bool ok = forge_editor_menus_define(&title, &items, &err);
    menu_items_free(&items);
    return ok ? 0 : raise(L, &err);
}

/* menus.extend(title, items) */
static int menus_extend(lua_State *L) {
    luaL_checkstring(L, 1);
    forge_editor_menus_list_item_t items = menu_items(L, 2);
    plugin_string_t title = lstr(L, 1), err;
    bool ok = forge_editor_menus_extend(&title, &items, &err);
    menu_items_free(&items);
    return ok ? 0 : raise(L, &err);
}

/* menus.remove(title) */
static int menus_remove(lua_State *L) {
    luaL_checkstring(L, 1);
    plugin_string_t title = lstr(L, 1), err;
    bool ok = forge_editor_menus_remove(&title, &err);
    return ok ? 0 : raise(L, &err);
}

/* menus.set_enabled(bool) */
static int menus_set_enabled(lua_State *L) {
    forge_editor_menus_set_enabled(lua_toboolean(L, 1));
    return 0;
}

/* menus.enabled() */
static int menus_enabled(lua_State *L) {
    lua_pushboolean(L, forge_editor_menus_enabled());
    return 1;
}

/* menus.open(title) -- "" closes. */
static int menus_open(lua_State *L) {
    plugin_string_t title = lstr(L, 1);
    forge_editor_menus_open(&title);
    return 0;
}

static int menus_close(lua_State *L) {
    forge_editor_menus_close();
    return 0;
}

/* menus.open_title() -> title or nil */
static int menus_open_title(lua_State *L) {
    plugin_string_t t;
    if (forge_editor_menus_open_title(&t)) {
        push_str(L, &t);
    } else {
        lua_pushnil(L);
    }
    return 1;
}

static int menus_choose(lua_State *L) {
    lua_pushboolean(L, forge_editor_menus_choose());
    return 1;
}

/* menus.selection() -> { menu =, label =, command =, index =, total =, available = } */
static int menus_selection(lua_State *L) {
    forge_editor_menus_item_selection_t s;
    if (!forge_editor_menus_selection(&s)) {
        lua_pushnil(L);
        return 1;
    }
    lua_createtable(L, 0, 6);
    lua_pushlstring(L, (const char *)s.menu.ptr, s.menu.len);
    lua_setfield(L, -2, "menu");
    lua_pushlstring(L, (const char *)s.label.ptr, s.label.len);
    lua_setfield(L, -2, "label");
    lua_pushlstring(L, (const char *)s.command.ptr, s.command.len);
    lua_setfield(L, -2, "command");
    lua_pushinteger(L, (lua_Integer)s.index);
    lua_setfield(L, -2, "index");
    lua_pushinteger(L, (lua_Integer)s.total);
    lua_setfield(L, -2, "total");
    lua_pushboolean(L, s.available);
    lua_setfield(L, -2, "available");
    forge_editor_menus_item_selection_free(&s);
    return 1;
}

/* completion.set_minibuffer_ui({ vertical = true, lines = 10 }) */
static int comp_minibuffer_ui(lua_State *L) {
    forge_editor_completion_set_minibuffer_ui(bool_field(L, 1, "vertical", true), (uint32_t)int_field(L, 1, "lines", 10));
    return 0;
}

/* completion.set_minibuffer_cycle({ cycle = true }) */
static int comp_minibuffer_cycle(lua_State *L) {
    forge_editor_completion_set_minibuffer_cycle(bool_field(L, 1, "cycle", false));
    return 0;
}

/* completion.set_matching({ "orderless", "prefix" }) */
static int comp_matching(lua_State *L) {
    luaL_checktype(L, 1, LUA_TTABLE);
    plugin_list_string_t l = string_list(L, 1);
    plugin_string_t err;
    bool ok = forge_editor_completion_set_matching(&l, &err);
    free(l.ptr);
    return ok ? 0 : raise(L, &err);
}

/* completion.set_sorting({ "history-length", "length" }) */
static int comp_sorting(lua_State *L) {
    luaL_checktype(L, 1, LUA_TTABLE);
    plugin_list_string_t l = string_list(L, 1);
    forge_editor_completion_set_sorting(&l);
    free(l.ptr);
    return 0;
}

/* completion.selected() -> { text =, annotation =, category =, index =, total = }
 * nil when no list is open. */
static int comp_selected(lua_State *L) {
    forge_editor_completion_selection_t s;
    if (!forge_editor_completion_selected(&s)) {
        lua_pushnil(L);
        return 1;
    }
    lua_createtable(L, 0, 5);
    lua_pushlstring(L, (const char *)s.text.ptr, s.text.len);
    lua_setfield(L, -2, "text");
    if (s.annotation.is_some) {
        lua_pushlstring(L, (const char *)s.annotation.val.ptr, s.annotation.val.len);
    } else {
        lua_pushnil(L);
    }
    lua_setfield(L, -2, "annotation");
    lua_pushlstring(L, (const char *)s.category.ptr, s.category.len);
    lua_setfield(L, -2, "category");
    lua_pushinteger(L, (lua_Integer)s.index);
    lua_setfield(L, -2, "index");
    lua_pushinteger(L, (lua_Integer)s.total);
    lua_setfield(L, -2, "total");
    forge_editor_completion_selection_free(&s);
    return 1;
}

/* completion.set_popup_frame({ min_width =, max_width =, border = }) */
static int comp_popup_frame(lua_State *L) {
    forge_editor_completion_set_popup_frame((uint32_t)int_field(L, 1, "min_width", 0),
                                            (uint32_t)int_field(L, 1, "max_width", 0),
                                            (uint32_t)int_field(L, 1, "border", 0));
    return 0;
}

/* completion.annotate(category, fn(candidates) -> { annotation, ... }) */
static int comp_annotate(lua_State *L) {
    luaL_checkstring(L, 1);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    lua_pushvalue(L, 1);
    keep(annotators_ref, L, 2);
    plugin_string_t c = lstr(L, 1);
    forge_editor_completion_annotate_category(&c);
    return 0;
}

/* completion.group(category, fn(candidates) -> { group, ... }) */
static int comp_group(lua_State *L) {
    luaL_checkstring(L, 1);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    lua_pushvalue(L, 1);
    keep(groupers_ref, L, 2);
    plugin_string_t c = lstr(L, 1);
    forge_editor_completion_group(&c);
    return 0;
}

/* completion.set_group_format("-- %s --") */
static int comp_group_format(lua_State *L) {
    plugin_string_t f = lstr(L, 1);
    forge_editor_completion_set_group_format(&f);
    return 0;
}

/* completion.set_layout("vertical", 10) */
static const char *const LAYOUTS[] = {"vertical", "flat", "grid", "reverse", "unobtrusive", "buffer", NULL};
static int comp_layout(lua_State *L) {
    int layout = luaL_checkoption(L, 1, NULL, LAYOUTS);
    uint32_t lines = (uint32_t)luaL_optinteger(L, 2, 10);
    forge_editor_completion_set_layout((forge_editor_completion_layout_t)layout, lines);
    return 0;
}

/* completion.set_row_labels({ "a", "s", "d" }) */
static int comp_row_labels(lua_State *L) {
    luaL_checktype(L, 1, LUA_TTABLE);
    plugin_list_string_t l = string_list(L, 1);
    forge_editor_completion_set_row_labels(&l);
    free(l.ptr);
    return 0;
}

/* completion.jump_to_row(row, accept) */
static int comp_jump_to_row(lua_State *L) {
    forge_editor_completion_jump_to_row((uint32_t)luaL_checkinteger(L, 1), lua_toboolean(L, 2));
    return 0;
}

/* completion.sessions() -> { { category =, prompt =, input =, total = }, ... } */
static int comp_sessions(lua_State *L) {
    forge_editor_completion_list_prompt_session_t l;
    forge_editor_completion_sessions(&l);
    lua_createtable(L, (int)l.len, 0);
    for (size_t i = 0; i < l.len; i++) {
        forge_editor_completion_prompt_session_t *s = &l.ptr[i];
        lua_createtable(L, 0, 4);
        lua_pushlstring(L, (const char *)s->category.ptr, s->category.len);
        lua_setfield(L, -2, "category");
        lua_pushlstring(L, (const char *)s->prompt.ptr, s->prompt.len);
        lua_setfield(L, -2, "prompt");
        lua_pushlstring(L, (const char *)s->input.ptr, s->input.len);
        lua_setfield(L, -2, "input");
        lua_pushinteger(L, (lua_Integer)s->total);
        lua_setfield(L, -2, "total");
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    forge_editor_completion_list_prompt_session_free(&l);
    return 1;
}

/* completion.resume(index) */
static int comp_resume(lua_State *L) {
    forge_editor_completion_resume((uint32_t)luaL_checkinteger(L, 1));
    return 0;
}

/* ---------------------------------------------------------- minibuffer */

static int mini_set_input(lua_State *L) {
    plugin_string_t t = lstr(L, 1);
    forge_editor_minibuffer_set_input(&t);
    return 0;
}

static int mini_on_open(lua_State *L) {
    plugin_string_t c = lstr(L, 1);
    forge_editor_minibuffer_on_open(&c);
    return 0;
}

static int mini_suspend(lua_State *L) {
    forge_editor_minibuffer_suspend();
    return 0;
}

static int mini_active(lua_State *L) {
    lua_pushboolean(L, forge_editor_minibuffer_active());
    return 1;
}

static int mini_input(lua_State *L) {
    plugin_string_t s;
    forge_editor_minibuffer_input(&s);
    push_str(L, &s);
    return 1;
}

static int mini_history_kind(lua_State *L) {
    plugin_string_t s;
    if (!forge_editor_minibuffer_history_kind(&s)) {
        lua_pushnil(L);
        return 1;
    }
    push_str(L, &s);
    return 1;
}

/* completion.set_at_point_ui({ popup =, auto =, delay =, min_prefix =, lines = }) */
static int comp_at_point_ui(lua_State *L) {
    forge_editor_completion_set_at_point_ui(bool_field(L, 1, "popup", true), bool_field(L, 1, "auto", false),
                                            (uint32_t)int_field(L, 1, "delay", 200),
                                            (uint32_t)int_field(L, 1, "min_prefix", 3),
                                            (uint32_t)int_field(L, 1, "lines", 10));
    return 0;
}

/* completion.source(name, fn(buffer, line, col, reply) [, { modes = { ... } }])
 * reply(l1, c1, l2, c2, { "text" or { text =, annotation = }, ... }) */
static int comp_source(lua_State *L) {
    luaL_checkstring(L, 1);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    plugin_string_t n = lstr(L, 1), err;
    plugin_list_string_t modes = {NULL, 0};
    if (lua_type(L, 3) == LUA_TTABLE) {
        lua_getfield(L, 3, "modes");
        modes = string_list(L, lua_gettop(L));
    }
    bool ok = forge_editor_completion_register_source(&n, &modes, &err);
    free(modes.ptr);
    if (!ok) return raise(L, &err);
    lua_pushvalue(L, 1);
    keep(sources_ref, L, 2);
    return 0;
}

/* The reply function handed to a source: upvalue 1 is the request id. */
static int comp_reply(lua_State *L) {
    uint32_t request = (uint32_t)lua_tointeger(L, lua_upvalueindex(1));
    forge_editor_types_span_t span = lspan(L, 1);
    luaL_checktype(L, 5, LUA_TTABLE);
    size_t n = lua_rawlen(L, 5);
    forge_editor_completion_candidate_t *c = calloc(n ? n : 1, sizeof *c);
    for (size_t i = 0; i < n; i++) {
        luaL_checkstack(L, 3, "too many completion candidates");
        lua_rawgeti(L, 5, (lua_Integer)i + 1);
        int t = lua_gettop(L);
        if (lua_istable(L, t)) {
            lua_getfield(L, t, "text");
            lua_getfield(L, t, "annotation");
            size_t len;
            const char *s = lua_tolstring(L, -2, &len);
            c[i].text.ptr = (uint8_t *)(s ? s : "");
            c[i].text.len = s ? len : 0;
            if (lua_isstring(L, -1)) {
                s = lua_tolstring(L, -1, &len);
                c[i].annotation.is_some = true;
                c[i].annotation.val.ptr = (uint8_t *)s;
                c[i].annotation.val.len = len;
            }
            /* keep the strings alive until the call: leave them on the stack */
        } else {
            size_t len;
            const char *s = lua_tolstring(L, t, &len);
            c[i].text.ptr = (uint8_t *)(s ? s : "");
            c[i].text.len = s ? len : 0;
        }
    }
    forge_editor_completion_list_candidate_t list = {c, n};
    forge_editor_completion_deliver(request, &span, &list);
    free(c);
    return 0;
}

/* ---------------------------------------------------------- processes */

/* process.spawn(program, args, { dir =, on_output = fn(data), on_exit = fn(code) })
 * -> id, or nil and an error. */
static int proc_spawn(lua_State *L) {
    plugin_string_t prog = lstr(L, 1), dir, err;
    size_t n = 0;
    plugin_string_t *args = NULL;
    if (!lua_isnoneornil(L, 2)) {
        luaL_checktype(L, 2, LUA_TTABLE);
        n = lua_rawlen(L, 2);
        args = calloc(n ? n : 1, sizeof *args);
        for (size_t i = 0; i < n; i++) {
            lua_rawgeti(L, 2, (lua_Integer)i + 1);
            size_t len;
            const char *a = lua_tolstring(L, -1, &len);
            if (!a) { free(args); return luaL_error(L, "process.spawn: argument %d isn't a string", (int)i + 1); }
            args[i].ptr = (uint8_t *)a;
            args[i].len = len;
            lua_pop(L, 1);
        }
    }
    const char *d = opt_field(L, 3, "dir");
    if (d) dir = str(d);
    plugin_list_string_t list = {args, n};
    uint32_t id;
    bool ok = forge_editor_processes_spawn(&prog, &list, d ? &dir : NULL, &id, &err);
    free(args);
    if (!ok) {
        lua_pushnil(L);
        push_str(L, &err);
        return 2;
    }
    /* Keep the handlers: processes[id] = opts table. */
    lua_rawgeti(L, LUA_REGISTRYINDEX, processes_ref);
    if (lua_istable(L, 3)) lua_pushvalue(L, 3); else lua_newtable(L);
    lua_rawseti(L, -2, id);
    lua_pop(L, 1);
    lua_pushinteger(L, id);
    return 1;
}

static int proc_write(lua_State *L) {
    uint32_t id = (uint32_t)luaL_checkinteger(L, 1);
    size_t len;
    const char *data = luaL_checklstring(L, 2, &len);
    plugin_list_u8_t bytes = {(uint8_t *)data, len};
    plugin_string_t err;
    if (!forge_editor_processes_write(id, &bytes, &err)) return raise(L, &err);
    return 0;
}

static int proc_close_input(lua_State *L) {
    forge_editor_processes_close_input((uint32_t)luaL_checkinteger(L, 1));
    return 0;
}

static int proc_kill(lua_State *L) {
    forge_editor_processes_kill((uint32_t)luaL_checkinteger(L, 1));
    return 0;
}

static int proc_project_root(lua_State *L) {
    plugin_string_t p = lstr(L, 1), root;
    if (forge_editor_processes_project_root(&p, &root)) push_str(L, &root); else lua_pushnil(L);
    return 1;
}

/* ------------------------------------------------- environment, system */

/* forge.system.*: where Forge runs. Adaptive, so no permission needed. */
static int sys_type(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_system_type(&s);
    push_str(L, &s);
    return 1;
}

static int sys_configuration(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_system_configuration(&s);
    push_str(L, &s);
    return 1;
}

static int sys_name(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_system_name(&s);
    push_str(L, &s);
    return 1;
}

static int sys_pid(lua_State *L) {
    lua_pushinteger(L, (lua_Integer)forge_editor_environment_pid());
    return 1;
}

static int sys_null_device(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_null_device(&s);
    push_str(L, &s);
    return 1;
}

static int sys_invocation_name(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_invocation_name(&s);
    push_str(L, &s);
    return 1;
}

/* forge.env.path_separator(): ":" or ";". A machine fact, not a secret. */
static int env_path_separator(lua_State *L) {
    plugin_string_t s;
    forge_editor_environment_path_separator(&s);
    push_str(L, &s);
    return 1;
}

/* forge.env.parse_path(text): the directories in a path list. An empty
 * component (the current directory) is "" here, not init.rhai's false. */
static int env_parse_path(lua_State *L) {
    plugin_string_t p = lstr(L, 1);
    plugin_list_string_t l;
    forge_editor_environment_parse_path(&p, &l);
    push_strings(L, &l);
    return 1;
}

/* forge.env.get(name): the value, or nil. Raises without the `environment`
 * permission (`environment = true` in the manifest, or forge.plugin.grant
 * in init.rhai). */
static int env_get(lua_State *L) {
    plugin_string_t n = lstr(L, 1), err;
    plugin_option_string_t ret;
    if (!forge_editor_environment_get(&n, &ret, &err)) return raise(L, &err);
    if (ret.is_some) push_str(L, &ret.val); else lua_pushnil(L);
    return 1;
}

/* forge.env.all(): every "KEY=VALUE", in order, like init.rhai's
 * process-environment. Raises without the permission. */
static int env_all(lua_State *L) {
    plugin_list_tuple2_string_string_t l;
    plugin_string_t err;
    if (!forge_editor_environment_variables(&l, &err)) return raise(L, &err);
    lua_createtable(L, (int)l.len, 0);
    for (size_t i = 0; i < l.len; i++) {
        lua_pushlstring(L, (const char *)l.ptr[i].f0.ptr, l.ptr[i].f0.len);
        lua_pushliteral(L, "=");
        lua_pushlstring(L, (const char *)l.ptr[i].f1.ptr, l.ptr[i].f1.len);
        lua_concat(L, 3);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_tuple2_string_string_free(&l);
    return 1;
}

/* ------------------------------------------------------------- setup */

static void lib(lua_State *L, const char *name, const luaL_Reg *fns) {
    lua_newtable(L);
    luaL_setfuncs(L, fns, 0);
    lua_setfield(L, -2, name);
}
#define LIB(name, ...) do { static const luaL_Reg fns[] = {__VA_ARGS__, {NULL, NULL}}; lib(L, name, fns); } while (0)

static void open_forge(lua_State *L) {
    luaL_newmetatable(L, BUFFER);
    luaL_newlib(L, buffer_methods);
    lua_setfield(L, -2, "__index");
    lua_pushcfunction(L, b_eq);
    lua_setfield(L, -2, "__eq");
    lua_pushcfunction(L, b_tostring);
    lua_setfield(L, -2, "__tostring");
    lua_pop(L, 1);

    lua_newtable(L);
    LIB("buffer", {"current", buf_current}, {"all", buf_all}, {"find", buf_find}, {"from_id", buf_from_id},
        {"create", buf_create}, {"visit", buf_visit});
    LIB("command", {"create", cmd_create}, {"run", cmd_run}, {"exists", cmd_exists}, {"doc", cmd_doc});
    LIB("keymap", {"set", km_set}, {"del", km_del}, {"remap", km_remap}, {"get", km_get}, {"is_prefix", km_is_prefix},
        {"execute", km_execute}, {"where_is", km_where_is});
    LIB("menus", {"define", menus_define}, {"extend", menus_extend}, {"remove", menus_remove},
        {"set_enabled", menus_set_enabled}, {"enabled", menus_enabled}, {"open", menus_open},
        {"close", menus_close}, {"open_title", menus_open_title}, {"choose", menus_choose},
        {"selection", menus_selection});

    LIB("completion", {"set_minibuffer_ui", comp_minibuffer_ui}, {"set_minibuffer_cycle", comp_minibuffer_cycle},
        {"set_matching", comp_matching},
        {"annotate", comp_annotate}, {"group", comp_group}, {"set_group_format", comp_group_format},
        {"set_layout", comp_layout}, {"set_row_labels", comp_row_labels}, {"jump_to_row", comp_jump_to_row},
        {"sessions", comp_sessions}, {"resume", comp_resume},
        {"set_at_point_ui", comp_at_point_ui}, {"source", comp_source},
        {"set_sorting", comp_sorting}, {"selected", comp_selected},
        {"set_popup_frame", comp_popup_frame});
    LIB("minibuffer", {"set_input", mini_set_input}, {"on_open", mini_on_open}, {"suspend", mini_suspend},
        {"active", mini_active}, {"input", mini_input}, {"history_kind", mini_history_kind});
    LIB("process", {"spawn", proc_spawn}, {"write", proc_write}, {"close_input", proc_close_input},
        {"kill", proc_kill}, {"project_root", proc_project_root});
    LIB("ui", {"input", ui_input}, {"select", ui_select}, {"confirm", ui_confirm}, {"read_key", ui_read_key});
    LIB("mode", {"define_major", mode_define_major}, {"define_minor", mode_define_minor},
        {"major_modes", mode_major_modes}, {"minor_modes", mode_minor_modes}, {"set_lighter", mode_set_lighter});
    LIB("system", {"type", sys_type}, {"configuration", sys_configuration}, {"name", sys_name},
        {"pid", sys_pid}, {"null_device", sys_null_device}, {"invocation_name", sys_invocation_name});
    LIB("env", {"get", env_get}, {"all", env_all}, {"path_separator", env_path_separator},
        {"parse_path", env_parse_path});
    lua_pushcfunction(L, f_notify);
    lua_setfield(L, -2, "notify");
    lua_pushcfunction(L, f_log);
    lua_setfield(L, -2, "log");
    lua_pushcfunction(L, f_on);
    lua_setfield(L, -2, "on");
    lua_setglobal(L, "forge");
    lua_pushcfunction(L, f_print);
    lua_setglobal(L, "print");

    lua_newtable(L);
    commands_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    answers_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    handlers_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    processes_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    annotators_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    groupers_ref = luaL_ref(L, LUA_REGISTRYINDEX);
    lua_newtable(L);
    sources_ref = luaL_ref(L, LUA_REGISTRYINDEX);
}

static int traceback(lua_State *L) {
    luaL_traceback(L, L, lua_tostring(L, 1), 1);
    return 1;
}

/* Call the function below the top `nargs` values. An error (with a
 * traceback) goes into `err` if given, else to the log. */
/* The string at `i` as a borrowed plugin string. */
static plugin_string_t forge_str_of(lua_State *L, int i) {
    size_t len = 0;
    const char *s = lua_tolstring(L, i, &len);
    plugin_string_t r = {(uint8_t *)(s ? s : "error"), s ? len : 5};
    return r;
}

static bool call(int nargs, plugin_string_t *err) {
    int base = lua_gettop(L) - nargs;
    lua_pushcfunction(L, traceback);
    lua_insert(L, base);
    int status = lua_pcall(L, nargs, 0, base);
    lua_remove(L, base);
    if (status == LUA_OK) return true;
    size_t len;
    const char *msg = lua_tolstring(L, -1, &len);
    if (!msg) msg = "error", len = 5;
    if (err) {
        plugin_string_dup_n(err, msg, len);
    } else {
        plugin_string_t m = {(uint8_t *)msg, len};
        forge_editor_log_log(FORGE_EDITOR_LOG_LEVEL_ERROR, &m);
    }
    lua_pop(L, 1);
    return false;
}

bool exports_plugin_init(plugin_string_t *err) {
    L = luaL_newstate();
    static const luaL_Reg libs[] = {{LUA_GNAME, luaopen_base},       {LUA_COLIBNAME, luaopen_coroutine},
                                    {LUA_TABLIBNAME, luaopen_table}, {LUA_STRLIBNAME, luaopen_string},
                                    {LUA_MATHLIBNAME, luaopen_math}, {LUA_UTF8LIBNAME, luaopen_utf8},
                                    {NULL, NULL}};
    for (const luaL_Reg *l = libs; l->func; l++) {
        luaL_requiref(L, l->name, l->func, 1);
        lua_pop(L, 1);
    }
    /* No loading code from files (there are none in the sandbox). */
    lua_pushnil(L);
    lua_setglobal(L, "dofile");
    lua_pushnil(L);
    lua_setglobal(L, "loadfile");
    open_forge(L);
    for (const struct forge_lua_script *s = forge_lua_scripts; s->name; s++) {
        char chunk[256];
        snprintf(chunk, sizeof chunk, "@%s", s->name);
        if (luaL_loadbufferx(L, s->source, s->len, chunk, "t") != LUA_OK) {
            size_t len;
            const char *msg = lua_tolstring(L, -1, &len);
            plugin_string_dup_n(err, msg, len);
            return false;
        }
        if (!call(0, err)) return false;
    }
    return true;
}

bool exports_forge_editor_handler_run_command(plugin_string_t *name, int64_t *maybe_prefix_arg, plugin_string_t *err) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, commands_ref);
    lua_pushlstring(L, (const char *)name->ptr, name->len);
    plugin_string_free(name);
    if (lua_rawget(L, -2) != LUA_TFUNCTION) {
        lua_pop(L, 2);
        plugin_string_dup(err, "this plugin has no such command");
        return false;
    }
    lua_remove(L, -2);
    if (maybe_prefix_arg) lua_pushinteger(L, *maybe_prefix_arg); else lua_pushnil(L);
    return call(1, err);
}

void exports_forge_editor_handler_on_event(exports_forge_editor_handler_event_t *event) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, handlers_ref);
    if (lua_rawgeti(L, -1, event->tag) == LUA_TTABLE) {
        lua_Integer n = (lua_Integer)lua_rawlen(L, -1);
        for (lua_Integer i = 1; i <= n; i++) {
            lua_rawgeti(L, -1, i);
            push_buffer(L, event->val.buffer_opened);
            call(1, NULL);
        }
    }
    lua_pop(L, 2);
}

void exports_forge_editor_handler_on_answer(exports_forge_editor_handler_request_t request, plugin_string_t *maybe_answer) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, answers_ref);
    lua_rawgeti(L, -1, request);
    lua_pushnil(L);
    lua_rawseti(L, -3, request);
    lua_remove(L, -2);
    if (lua_istable(L, -1)) { /* ui.confirm */
        lua_getfield(L, -1, "confirm");
        lua_remove(L, -2);
        lua_pushboolean(L, maybe_answer && maybe_answer->len == 1 && maybe_answer->ptr[0] == 'y');
    } else if (lua_isfunction(L, -1)) {
        if (maybe_answer) lua_pushlstring(L, (const char *)maybe_answer->ptr, maybe_answer->len); else lua_pushnil(L);
    } else {
        lua_pop(L, 1);
        if (maybe_answer) plugin_string_free(maybe_answer);
        return;
    }
    if (maybe_answer) plugin_string_free(maybe_answer);
    call(1, NULL);
}

/* The process's handler `field` from its spawn options, pushed (or
 * nothing: returns false). */
static bool process_handler(uint32_t id, const char *field) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, processes_ref);
    if (lua_rawgeti(L, -1, id) != LUA_TTABLE) {
        lua_pop(L, 2);
        return false;
    }
    lua_getfield(L, -1, field);
    lua_remove(L, -2);
    lua_remove(L, -2);
    if (!lua_isfunction(L, -1)) {
        lua_pop(L, 1);
        return false;
    }
    return true;
}

void exports_forge_editor_process_handler_on_output(exports_forge_editor_process_handler_process_t process, plugin_list_u8_t *data) {
    if (process_handler(process, "on_output")) {
        lua_pushlstring(L, (const char *)data->ptr, data->len);
        call(1, NULL);
    }
    plugin_list_u8_free(data);
}

void exports_forge_editor_process_handler_on_exit(exports_forge_editor_process_handler_process_t process, int32_t *maybe_code) {
    if (process_handler(process, "on_exit")) {
        if (maybe_code) lua_pushinteger(L, *maybe_code); else lua_pushnil(L);
        call(1, NULL);
    }
    lua_rawgeti(L, LUA_REGISTRYINDEX, processes_ref);
    lua_pushnil(L);
    lua_rawseti(L, -2, process);
    lua_pop(L, 1);
}

void exports_forge_editor_completion_handler_annotate(plugin_string_t *category, plugin_list_string_t *candidates,
                                                      plugin_list_string_t *ret) {
    ret->ptr = NULL;
    ret->len = 0;
    lua_rawgeti(L, LUA_REGISTRYINDEX, annotators_ref);
    lua_pushlstring(L, (const char *)category->ptr, category->len);
    plugin_string_free(category);
    lua_rawget(L, -2);
    lua_remove(L, -2);
    size_t n = candidates->len;
    if (!lua_isfunction(L, -1)) {
        lua_pop(L, 1);
        plugin_list_string_free(candidates);
        return;
    }
    lua_createtable(L, (int)n, 0);
    for (size_t i = 0; i < n; i++) {
        lua_pushlstring(L, (const char *)candidates->ptr[i].ptr, candidates->ptr[i].len);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_string_free(candidates);
    int base = lua_gettop(L) - 1;
    lua_pushcfunction(L, traceback);
    lua_insert(L, base);
    if (lua_pcall(L, 1, 1, base) != LUA_OK) {
        plugin_string_t m = forge_str_of(L, -1);
        forge_editor_log_log(FORGE_EDITOR_LOG_LEVEL_ERROR, &m);
        lua_pop(L, 2);
        return;
    }
    lua_remove(L, base);
    if (lua_istable(L, -1)) {
        ret->len = n;
        ret->ptr = calloc(n ? n : 1, sizeof *ret->ptr);
        for (size_t i = 0; i < n; i++) {
            lua_rawgeti(L, -1, (lua_Integer)i + 1);
            size_t len = 0;
            const char *s = lua_tolstring(L, -1, &len);
            plugin_string_dup_n(&ret->ptr[i], s ? s : "", s ? len : 0);
            lua_pop(L, 1);
        }
    }
    lua_pop(L, 1);
}

void exports_forge_editor_completion_handler_group(plugin_string_t *category, plugin_list_string_t *candidates,
                                                   plugin_list_string_t *ret) {
    ret->ptr = NULL;
    ret->len = 0;
    lua_rawgeti(L, LUA_REGISTRYINDEX, groupers_ref);
    lua_pushlstring(L, (const char *)category->ptr, category->len);
    plugin_string_free(category);
    lua_rawget(L, -2);
    lua_remove(L, -2);
    size_t n = candidates->len;
    if (!lua_isfunction(L, -1)) {
        lua_pop(L, 1);
        plugin_list_string_free(candidates);
        return;
    }
    lua_createtable(L, (int)n, 0);
    for (size_t i = 0; i < n; i++) {
        lua_pushlstring(L, (const char *)candidates->ptr[i].ptr, candidates->ptr[i].len);
        lua_rawseti(L, -2, (lua_Integer)i + 1);
    }
    plugin_list_string_free(candidates);
    int base = lua_gettop(L) - 1;
    lua_pushcfunction(L, traceback);
    lua_insert(L, base);
    if (lua_pcall(L, 1, 1, base) != LUA_OK) {
        plugin_string_t m = forge_str_of(L, -1);
        forge_editor_log_log(FORGE_EDITOR_LOG_LEVEL_ERROR, &m);
        lua_pop(L, 2);
        return;
    }
    lua_remove(L, base);
    if (lua_istable(L, -1)) {
        ret->len = n;
        ret->ptr = calloc(n ? n : 1, sizeof *ret->ptr);
        for (size_t i = 0; i < n; i++) {
            lua_rawgeti(L, -1, (lua_Integer)i + 1);
            size_t len = 0;
            const char *s = lua_tolstring(L, -1, &len);
            plugin_string_dup_n(&ret->ptr[i], s ? s : "", s ? len : 0);
            lua_pop(L, 1);
        }
    }
    lua_pop(L, 1);
}

void exports_forge_editor_completion_handler_complete(uint32_t request, plugin_string_t *source, uint32_t buffer,
                                                      exports_forge_editor_completion_handler_position_t *point) {
    lua_rawgeti(L, LUA_REGISTRYINDEX, sources_ref);
    lua_pushlstring(L, (const char *)source->ptr, source->len);
    plugin_string_free(source);
    lua_rawget(L, -2);
    lua_remove(L, -2);
    if (!lua_isfunction(L, -1)) {
        lua_pop(L, 1);
        return;
    }
    push_buffer(L, buffer);
    push_pos(L, *point);
    lua_pushinteger(L, request);
    lua_pushcclosure(L, comp_reply, 1);
    call(4, NULL);
}

void exports_forge_editor_completion_handler_commit(plugin_string_t *candidate, plugin_string_t *status) {
    plugin_string_free(candidate);
    plugin_string_free(status);
}

void exports_forge_editor_completion_handler_resolve_location(
    exports_forge_editor_completion_handler_selection_t *selection) {
    exports_forge_editor_completion_handler_selection_free(selection);
}

void exports_forge_editor_http_handler_on_response(
    exports_forge_editor_http_handler_http_request_t request,
    exports_forge_editor_http_handler_result_response_string_t *answer) {
    (void)request;
    exports_forge_editor_http_handler_result_response_string_free(answer);
}
