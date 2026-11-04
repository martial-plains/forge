#include <stdint.h>
#include <stddef.h>
/* setjmp/longjmp on top of Wasm exception handling (as in wasi-libc). */
struct jmp_buf_impl {
    void *func_invocation_id;
    uint32_t label;
    struct arg { void *env; int val; } arg;
};
void __wasm_setjmp(void *env, uint32_t label, void *func_invocation_id) {
    struct jmp_buf_impl *jb = env;
    if (label == 0 || func_invocation_id == NULL) __builtin_trap();
    jb->func_invocation_id = func_invocation_id;
    jb->label = label;
}
uint32_t __wasm_setjmp_test(void *env, void *func_invocation_id) {
    struct jmp_buf_impl *jb = env;
    if (jb->label == 0 || func_invocation_id == NULL) __builtin_trap();
    return jb->func_invocation_id == func_invocation_id ? jb->label : 0;
}
void __wasm_longjmp(void *env, int val) {
    struct jmp_buf_impl *jb = env;
    struct arg *arg = &jb->arg;
    if (val == 0) val = 1;
    arg->env = env;
    arg->val = val;
    __builtin_wasm_throw(1, arg);
}
/*
 * The throw above references the WebAssembly tag `__c_longjmp`, which the
 * C compiler can't define but inline assembly can. LLVM 22 started
 * requiring this (before, the tag was implicit); the same as wasi-libc's
 * setjmp runtime.
 */
#if defined(__clang__) && __clang_major__ >= 22
__asm__(".globl __c_longjmp\n"
#if defined(__wasm32__)
        ".tagtype __c_longjmp i32\n"
#elif defined(__wasm64__)
        ".tagtype __c_longjmp i64\n"
#else
#error "unsupported wasm architecture"
#endif
        "__c_longjmp:\n");
#endif
