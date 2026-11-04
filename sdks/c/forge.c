/* The plumbing behind forge.h: the exports Forge calls, dispatching to the
 * functions registered with forge_command, forge_read_string, forge_on. */
#include "forge.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

plugin_string_t forge_str(const char *s) {
    plugin_string_t r;
    plugin_string_set(&r, s);
    return r;
}

char *forge_cstr(const plugin_string_t *s) {
    char *c = malloc(s->len + 1);
    memcpy(c, s->ptr, s->len);
    c[s->len] = 0;
    return c;
}

/* ---------------------------------------------------------- errors */

static char last_error[512];

const char *forge_error(void) { return last_error; }

void forge_fail(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(last_error, sizeof last_error, fmt, ap);
    va_end(ap);
}

/* Keep Forge's error message; true if there was none. */
static bool check(bool ok, plugin_string_t *err) {
    if (!ok) {
        snprintf(last_error, sizeof last_error, "%.*s", (int)err->len, (const char *)err->ptr);
        plugin_string_free(err);
    }
    return ok;
}

/* ------------------------------------------------------- registries */

struct command { char *name; forge_command_fn run; };
struct answer { uint32_t request; forge_answer_fn then; void *data; bool y_or_n; };
struct handler { forge_editor_events_kind_t kind; forge_event_fn f; };

static struct command *commands;
static size_t n_commands;
static struct answer *answers;
static size_t n_answers;
static struct handler *handlers;
static size_t n_handlers;

#define PUSH(arr, n, item)                              \
    do {                                                \
        (arr) = realloc((arr), ((n) + 1) * sizeof *(arr)); \
        (arr)[(n)++] = (item);                          \
    } while (0)

static bool add_command(const char *name, const char *doc, forge_command_fn run, bool background) {
    plugin_string_t n = forge_str(name), d = forge_str(doc), err;
    bool ok = background ? forge_editor_commands_register_background(&n, &d, &err)
                         : forge_editor_commands_register(&n, &d, &err);
    if (!check(ok, &err)) return false;
    struct command c = {strdup(name), run};
    PUSH(commands, n_commands, c);
    return true;
}

bool forge_command(const char *name, const char *doc, forge_command_fn run) {
    return add_command(name, doc, run, false);
}

bool forge_command_background(const char *name, const char *doc, forge_command_fn run) {
    return add_command(name, doc, run, true);
}

static bool bind(forge_editor_keymap_scope_t *scope, const char *keys, const char *command) {
    plugin_string_t k = forge_str(keys), c = forge_str(command), err;
    return check(forge_editor_keymap_bind(scope, &k, &c, &err), &err);
}

bool forge_bind(const char *keys, const char *command) {
    forge_editor_keymap_scope_t scope = {.tag = FORGE_EDITOR_KEYMAP_SCOPE_GLOBAL};
    return bind(&scope, keys, command);
}

bool forge_bind_in_mode(const char *mode, const char *keys, const char *command) {
    forge_editor_keymap_scope_t scope = {.tag = FORGE_EDITOR_KEYMAP_SCOPE_MODE};
    scope.val.mode = forge_str(mode);
    return bind(&scope, keys, command);
}

bool forge_define_minor_mode(const char *name) {
    plugin_string_t n = forge_str(name), err;
    return check(forge_editor_modes_define_minor_mode(&n, &err), &err);
}

bool forge_run_command(const char *name) {
    plugin_string_t n = forge_str(name), err;
    return check(forge_editor_commands_run(&n, &err), &err);
}

void forge_read_string(const char *prompt, const char *initial, forge_answer_fn then, void *data) {
    plugin_string_t p = forge_str(prompt), i = forge_str(initial ? initial : "");
    struct answer a = {forge_editor_minibuffer_read_string(&p, &i), then, data, false};
    PUSH(answers, n_answers, a);
}

void forge_y_or_n(const char *prompt, forge_answer_fn then, void *data) {
    plugin_string_t p = forge_str(prompt);
    struct answer a = {forge_editor_minibuffer_y_or_n(&p), then, data, true};
    PUSH(answers, n_answers, a);
}

void forge_on(forge_editor_events_kind_t kind, forge_event_fn f) {
    forge_editor_events_subscribe(kind);
    struct handler h = {kind, f};
    PUSH(handlers, n_handlers, h);
}

/* ------------------------------------------------ messages, buffers */

static char *vformat(const char *fmt, va_list ap) {
    va_list ap2;
    va_copy(ap2, ap);
    int n = vsnprintf(NULL, 0, fmt, ap);
    char *s = malloc(n + 1);
    vsnprintf(s, n + 1, fmt, ap2);
    va_end(ap2);
    return s;
}

void forge_message(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    char *s = vformat(fmt, ap);
    va_end(ap);
    plugin_string_t t = forge_str(s);
    forge_editor_minibuffer_message(&t);
    free(s);
}

void forge_log(forge_editor_log_level_t level, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    char *s = vformat(fmt, ap);
    va_end(ap);
    plugin_string_t t = forge_str(s);
    forge_editor_log_log(level, &t);
    free(s);
}

void forge_insert(const char *text) {
    forge_editor_buffers_own_buffer_t b = forge_editor_buffers_static_buffer_current();
    plugin_string_t t = forge_str(text);
    forge_editor_buffers_method_buffer_insert(forge_editor_buffers_borrow_buffer(b), &t);
    forge_editor_buffers_buffer_drop_own(b);
}

void forge_insert_formatted(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    char *s = vformat(fmt, ap);
    va_end(ap);
    forge_insert(s);
    free(s);
}

char *forge_buffer_text(void) {
    forge_editor_buffers_own_buffer_t b = forge_editor_buffers_static_buffer_current();
    plugin_string_t t;
    forge_editor_buffers_method_buffer_text(forge_editor_buffers_borrow_buffer(b), &t);
    forge_editor_buffers_buffer_drop_own(b);
    char *c = forge_cstr(&t);
    plugin_string_free(&t);
    return c;
}

/* ----------------------------------------------- what Forge calls */

static void set_err(plugin_string_t *err) {
    if (!last_error[0]) snprintf(last_error, sizeof last_error, "failed");
    plugin_string_dup(err, last_error);
    last_error[0] = 0;
}

bool exports_plugin_init(plugin_string_t *err) {
    last_error[0] = 0;
    if (forge_plugin_init()) return true;
    set_err(err);
    return false;
}

bool exports_forge_editor_handler_run_command(plugin_string_t *name, int64_t *maybe_prefix_arg, plugin_string_t *err) {
    bool ok = false, found = false;
    last_error[0] = 0;
    for (size_t i = 0; i < n_commands; i++) {
        if (strlen(commands[i].name) == name->len && memcmp(commands[i].name, name->ptr, name->len) == 0) {
            found = true;
            ok = commands[i].run(maybe_prefix_arg);
            break;
        }
    }
    if (!found) forge_fail("this plugin has no command %.*s", (int)name->len, (const char *)name->ptr);
    plugin_string_free(name);
    if (!ok) set_err(err);
    return ok;
}

void exports_forge_editor_handler_on_event(exports_forge_editor_handler_event_t *event) {
    for (size_t i = 0; i < n_handlers; i++) {
        if (handlers[i].kind == event->tag) handlers[i].f(event->val.buffer_opened);
    }
}

void exports_forge_editor_handler_on_answer(exports_forge_editor_handler_request_t request, plugin_string_t *maybe_answer) {
    for (size_t i = 0; i < n_answers; i++) {
        if (answers[i].request != request) continue;
        struct answer a = answers[i];
        answers[i] = answers[--n_answers];
        char *s = maybe_answer ? forge_cstr(maybe_answer) : NULL;
        if (maybe_answer) plugin_string_free(maybe_answer);
        if (a.y_or_n) {
            a.then(s && strcmp(s, "y") == 0 ? "y" : NULL, a.data);
        } else {
            a.then(s, a.data);
        }
        free(s);
        return;
    }
    if (maybe_answer) plugin_string_free(maybe_answer);
}
