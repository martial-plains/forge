;; A test plugin for background calls (forge:editor 0.1.2). Commands,
;; both registered with commands.register-background:
;;   bg-insert: spin a while, then insert "[bg]" at point in the current
;;     buffer, put an "error" decoration on (0,0)-(0,4) and log "done".
;;   bg-bind: try keymap.bind (not allowed in the background) and log
;;     the error.
(module
      (import "forge:editor/log@0.1.2" "log" (func $log (param i32 i32 i32)))
      (import "forge:editor/commands@0.1.2" "register-background" (func $register_bg (param i32 i32 i32 i32 i32)))
      (import "forge:editor/keymap@0.1.2" "bind" (func $bind (param i32 i32 i32 i32 i32 i32 i32 i32)))
      (import "forge:editor/buffers@0.1.2" "[static]buffer.current" (func $current (result i32)))
      (import "forge:editor/buffers@0.1.2" "[method]buffer.insert" (func $insert (param i32 i32 i32)))
      (import "forge:editor/buffers@0.1.2" "[resource-drop]buffer" (func $drop (param i32)))
      (import "forge:editor/decorations@0.1.2" "set" (func $deco_set (param i32 i32 i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "bg-insert")           ;; 9
      (data (i32.const 120) "bg-bind")             ;; 7
      (data (i32.const 140) "<f6>")                ;; 4
      (data (i32.const 160) "[bg]")                ;; 4
      (data (i32.const 200) "error")               ;; 5
      (data (i32.const 230) "done")                ;; 4

      (global $heap (mut i32) (i32.const 4096))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p))

      (func (export "init") (result i32)
        (call $register_bg (i32.const 100) (i32.const 9) (i32.const 0) (i32.const 0) (i32.const 16))
        (call $register_bg (i32.const 120) (i32.const 7) (i32.const 0) (i32.const 0) (i32.const 16))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.2#run-command") (param $name i32) (param $len i32) (param i32 i64) (result i32)
        (local $b i32)
        (local $i i32)
        (if (i32.eq (local.get $len) (i32.const 9))
          (then
            ;; Work for a while (tens of milliseconds).
            (local.set $i (i32.const 30000000))
            (loop $spin
              (local.set $i (i32.sub (local.get $i) (i32.const 1)))
              (br_if $spin (local.get $i)))
            (local.set $b (call $current))
            (call $insert (local.get $b) (i32.const 160) (i32.const 4))
            ;; decoration (0,0)-(0,4) "error", no message
            (i32.store (i32.const 1000) (i32.const 0))
            (i32.store (i32.const 1004) (i32.const 0))
            (i32.store (i32.const 1008) (i32.const 0))
            (i32.store (i32.const 1012) (i32.const 4))
            (i32.store (i32.const 1016) (i32.const 200))
            (i32.store (i32.const 1020) (i32.const 5))
            (i32.store8 (i32.const 1024) (i32.const 0))
            (call $deco_set (local.get $b) (i32.const 1000) (i32.const 1) (i32.const 40))
            (call $drop (local.get $b))
            (call $log (i32.const 1) (i32.const 230) (i32.const 4)))
          (else
            ;; bind(global, "<f6>", "bg-insert") -> result at 40
            (call $bind (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 140) (i32.const 4) (i32.const 100) (i32.const 9) (i32.const 40))
            (if (i32.load8_u (i32.const 40))
              (then (call $log (i32.const 3) (i32.load (i32.const 44)) (i32.load (i32.const 48)))))))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.2#on-event") (param i32 i32))
      (func (export "forge:editor/handler@0.1.2#on-answer") (param i32 i32 i32 i32))
    )
