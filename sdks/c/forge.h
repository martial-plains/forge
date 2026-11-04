/* Write Forge plugins in C (or C++).
 *
 * A plugin is a WebAssembly component for the forge:editor/plugin world
 * (wit/forge-editor.wit). `wit-bindgen c` generates the raw API
 * (plugin.h: forge_editor_buffers_..., forge_editor_commands_...); this
 * header adds the plumbing that makes the callback style pleasant, with
 * plain C strings:
 *
 *     #include "forge.h"
 *
 *     static void got_name(const char *name, void *data) {
 *         if (name) forge_insert_formatted("Hello, %s!", name);
 *     }
 *     static bool greet(const int64_t *prefix) {
 *         forge_read_string("Name: ", "", got_name, NULL);
 *         return true;
 *     }
 *     bool forge_plugin_init(void) {
 *         return forge_command("greet", "Ask for a name and insert a greeting.", greet);
 *     }
 *
 * Build with ./build.sh plugin.c -o greet.wasm (see there). The manifest
 * (name, version, permissions) goes in forge.toml next to the sources;
 * build.sh embeds it in the component.
 *
 * Forge calls a plugin and waits for it to return; a plugin never waits
 * for the user. So questions take a function that runs when the answer
 * arrives, and prompts and messages show after the current call returns.
 * Functions that can fail return false; forge_error() then says why.
 */
#ifndef FORGE_H
#define FORGE_H

#include "plugin.h"

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Implement this: register commands, bind keys, subscribe to events.
 * Return false (after forge_fail) to refuse loading. */
bool forge_plugin_init(void);

/* A command. `prefix` is the prefix argument (C-u 3 -> 3, plain C-u -> 4)
 * or NULL. Return false after forge_fail("...") to report an error. */
typedef bool (*forge_command_fn)(const int64_t *prefix);
/* An answer: NULL if the user pressed C-g. */
typedef void (*forge_answer_fn)(const char *answer, void *data);
/* An event, with the buffer id. */
typedef void (*forge_event_fn)(uint32_t buffer_id);

bool forge_command(const char *name, const char *doc, forge_command_fn run);
/* A command that runs in the background, on a snapshot of the editor
 * (Forge 0.1.2; see commands.register-background in the WIT). */
bool forge_command_background(const char *name, const char *doc, forge_command_fn run);
bool forge_bind(const char *keys, const char *command);
bool forge_bind_in_mode(const char *mode, const char *keys, const char *command);
bool forge_define_minor_mode(const char *name);
/* Run a command after this call returns. */
bool forge_run_command(const char *name);

void forge_read_string(const char *prompt, const char *initial, forge_answer_fn then, void *data);
void forge_y_or_n(const char *prompt, forge_answer_fn then, void *data); /* answer "y" or NULL */
void forge_on(forge_editor_events_kind_t kind, forge_event_fn f);

/* Echo area and *Messages*, printf-style. */
void forge_message(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
void forge_log(forge_editor_log_level_t level, const char *fmt, ...) __attribute__((format(printf, 2, 3)));

/* The current buffer: insert at point; its whole text (free() it). */
void forge_insert(const char *text);
void forge_insert_formatted(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
char *forge_buffer_text(void);

/* Why the last call returned false; set it yourself with forge_fail. */
const char *forge_error(void);
void forge_fail(const char *fmt, ...) __attribute__((format(printf, 1, 2)));

/* Helpers for the raw API. */
plugin_string_t forge_str(const char *s);  /* borrows s */
char *forge_cstr(const plugin_string_t *s); /* malloc'd copy */

#ifdef __cplusplus
}
#endif

#endif
