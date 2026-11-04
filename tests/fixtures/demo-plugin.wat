;; A test plugin written directly against the canonical ABI (no Rust
;; wasm target needed). It registers demo-greet (asks for a name, then
;; inserts "Hi, NAME" into the current buffer), binds it to <f5>, defines
;; demo-mode, subscribes to buffer-opened and buffer-changed, and logs
;; each event's name. Used by the core and terminal tests.
(module
      (import "forge:editor/log@0.1.0" "log" (func $log (param i32 i32 i32)))
      (import "forge:editor/commands@0.1.0" "register" (func $register (param i32 i32 i32 i32 i32)))
      (import "forge:editor/keymap@0.1.0" "bind" (func $bind (param i32 i32 i32 i32 i32 i32 i32 i32)))
      (import "forge:editor/events@0.1.0" "subscribe" (func $subscribe (param i32)))
      (import "forge:editor/minibuffer@0.1.0" "read-string" (func $read (param i32 i32 i32 i32) (result i32)))
      (import "forge:editor/modes@0.1.0" "define-minor-mode" (func $minor (param i32 i32 i32)))
      (import "forge:editor/buffers@0.1.0" "[static]buffer.current" (func $current (result i32)))
      (import "forge:editor/buffers@0.1.0" "[method]buffer.insert" (func $insert (param i32 i32 i32)))
      (import "forge:editor/buffers@0.1.0" "[resource-drop]buffer" (func $drop (param i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "demo-greet")          ;; 10
      (data (i32.const 120) "Greet someone.")      ;; 14
      (data (i32.const 140) "<f5>")                ;; 4
      (data (i32.const 150) "Name: ")              ;; 6
      (data (i32.const 160) "Hi, ")                ;; 4
      (data (i32.const 170) "demo-mode")           ;; 9
      ;; event names at 300 + 20 * tag, lengths at 400 + 4 * tag
      (data (i32.const 300) "buffer-opened")
      (data (i32.const 320) "buffer-killed")
      (data (i32.const 340) "buffer-changed")
      (data (i32.const 360) "buffer-saved")
      (data (i32.const 380) "mode-changed")
      (data (i32.const 400) "\0d\00\00\00\0d\00\00\00\0e\00\00\00\0c\00\00\00\0c\00\00\00")

      (global $heap (mut i32) (i32.const 4096))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p))

      (func (export "init") (result i32)
        ;; commands.register("demo-greet", "Greet someone.") -> result at 16
        (call $register (i32.const 100) (i32.const 10) (i32.const 120) (i32.const 14) (i32.const 16))
        ;; keymap.bind(global, "<f5>", "demo-greet")
        (call $bind (i32.const 0) (i32.const 0) (i32.const 0)
                    (i32.const 140) (i32.const 4) (i32.const 100) (i32.const 10) (i32.const 16))
        ;; modes.define-minor-mode("demo-mode")
        (call $minor (i32.const 170) (i32.const 9) (i32.const 16))
        (call $subscribe (i32.const 0))   ;; buffer-opened
        (call $subscribe (i32.const 2))   ;; buffer-changed
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.0#run-command") (param i32 i32 i32 i64) (result i32)
        ;; minibuffer.read-string("Name: ", "") -> request id (ignored)
        (drop (call $read (i32.const 150) (i32.const 6) (i32.const 0) (i32.const 0)))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.0#on-event") (param $tag i32) (param $id i32)
        (call $log (i32.const 1)
          (i32.add (i32.const 300) (i32.mul (local.get $tag) (i32.const 20)))
          (i32.load (i32.add (i32.const 400) (i32.mul (local.get $tag) (i32.const 4))))))

      ;; on-answer(request, option<string>): (request, is-some, ptr, len)
      (func (export "forge:editor/handler@0.1.0#on-answer") (param i32 i32 i32 i32)
        (local $b i32)
        (if (local.get 1) (then
          (local.set $b (call $current))
          (call $insert (local.get $b) (i32.const 160) (i32.const 4))
          (call $insert (local.get $b) (local.get 2) (local.get 3))
          (call $drop (local.get $b)))))
    )
