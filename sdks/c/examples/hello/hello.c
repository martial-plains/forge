/* An example Forge plugin in C: the same as plugins/hello in Rust, plus
 * a word count.
 *
 *   ../../build.sh hello.c -o hello-c.wasm
 *   forge --plugin hello-c.wasm          (then C-c C-k)
 */
#include "forge.h"

#include <ctype.h>
#include <stdlib.h>

static void got_name(const char *name, void *data) {
    int times = (int)(intptr_t)data;
    for (int i = 0; name && i < times; i++) forge_insert_formatted("Hello, %s!\n", name);
}

static bool greet(const int64_t *prefix) {
    intptr_t times = prefix && *prefix > 1 ? (intptr_t)*prefix : 1;
    forge_read_string("Greet whom? ", "world", got_name, (void *)times);
    return true;
}

static bool count_words(const int64_t *prefix) {
    (void)prefix;
    char *text = forge_buffer_text();
    int words = 0;
    for (char *p = text, prev = ' '; *p; prev = *p++) {
        if (!isspace((unsigned char)*p) && isspace((unsigned char)prev)) words++;
    }
    free(text);
    forge_message("%d words", words);
    return true;
}

static void saved(uint32_t buffer_id) {
    forge_log(FORGE_EDITOR_LOG_LEVEL_INFO, "saved buffer %u", buffer_id);
}

bool forge_plugin_init(void) {
    if (!forge_command("hello-c-greet", "Ask for a name and insert a greeting at point.", greet)) return false;
    if (!forge_command("hello-c-count-words", "Show how many words the buffer has.", count_words)) return false;
    if (!forge_bind("C-c C-k", "hello-c-greet")) return false;
    forge_on(FORGE_EDITOR_EVENTS_KIND_BUFFER_SAVED, saved);
    forge_log(FORGE_EDITOR_LOG_LEVEL_INFO, "Hello from C! Try C-c C-k.");
    return true;
}
