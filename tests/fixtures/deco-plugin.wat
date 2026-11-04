;; A test plugin for what forge:editor 0.1.1 added: buffer options,
;; indent-line and the decorations interface. Written against the
;; canonical ABI like demo-plugin.wat. Commands:
;;   deco-run: in the current buffer, set line-numbers to relative,
;;     re-indent line 1, put an "error" decoration (message "bad") on
;;     line 0 columns 0-2, then log the syntax face found there.
;;   deco-bad-face: try the face "red" and log the error.
(module
      (import "forge:editor/log@0.1.1" "log" (func $log (param i32 i32 i32)))
      (import "forge:editor/commands@0.1.1" "register" (func $register (param i32 i32 i32 i32 i32)))
      (import "forge:editor/buffers@0.1.1" "[static]buffer.current" (func $current (result i32)))
      (import "forge:editor/buffers@0.1.1" "[method]buffer.set-option" (func $set_option (param i32 i32 i32 i32 i32 i32)))
      (import "forge:editor/buffers@0.1.1" "[method]buffer.indent-line" (func $indent (param i32 i32) (result i32)))
      (import "forge:editor/buffers@0.1.1" "[resource-drop]buffer" (func $drop (param i32)))
      (import "forge:editor/decorations@0.1.1" "set" (func $deco_set (param i32 i32 i32 i32)))
      (import "forge:editor/decorations@0.1.1" "syntax" (func $deco_syntax (param i32 i32 i32 i32 i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "deco-run")            ;; 8
      (data (i32.const 120) "deco-bad-face")       ;; 13
      (data (i32.const 140) "line-numbers")        ;; 12
      (data (i32.const 160) "relative")            ;; 8
      (data (i32.const 200) "error")               ;; 5
      (data (i32.const 210) "bad")                 ;; 3
      (data (i32.const 220) "red")                 ;; 3
      (data (i32.const 230) "indented")            ;; 8

      (global $heap (mut i32) (i32.const 4096))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p))

      (func (export "init") (result i32)
        (call $register (i32.const 100) (i32.const 8) (i32.const 0) (i32.const 0) (i32.const 16))
        (call $register (i32.const 120) (i32.const 13) (i32.const 0) (i32.const 0) (i32.const 16))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      ;; One decoration at 1000: span (0,0)-(0,2), face at $face/$len,
      ;; message "bad".
      (func $one_decoration (param $face i32) (param $len i32)
        (i32.store (i32.const 1000) (i32.const 0))
        (i32.store (i32.const 1004) (i32.const 0))
        (i32.store (i32.const 1008) (i32.const 0))
        (i32.store (i32.const 1012) (i32.const 2))
        (i32.store (i32.const 1016) (local.get $face))
        (i32.store (i32.const 1020) (local.get $len))
        (i32.store8 (i32.const 1024) (i32.const 1))
        (i32.store (i32.const 1028) (i32.const 210))
        (i32.store (i32.const 1032) (i32.const 3)))

      (func (export "forge:editor/handler@0.1.1#run-command") (param $name i32) (param $len i32) (param i32 i64) (result i32)
        (local $b i32)
        (local $list i32)
        (local.set $b (call $current))
        (if (i32.eq (local.get $len) (i32.const 8))
          (then
            ;; set-option("line-numbers", "relative") -> result at 16
            (call $set_option (local.get $b) (i32.const 140) (i32.const 12) (i32.const 160) (i32.const 8) (i32.const 16))
            ;; indent-line(1) -> log "indented" if it did
            (if (call $indent (local.get $b) (i32.const 1))
              (then (call $log (i32.const 1) (i32.const 230) (i32.const 8))))
            (call $one_decoration (i32.const 200) (i32.const 5))
            (call $deco_set (local.get $b) (i32.const 1000) (i32.const 1) (i32.const 40))
            ;; syntax(span (0,0)-(0,2)) -> list at 48; log the first face
            (call $deco_syntax (local.get $b) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 2) (i32.const 48))
            (local.set $list (i32.load (i32.const 48)))
            (if (i32.load (i32.const 52))
              (then (call $log (i32.const 1)
                (i32.load (i32.add (local.get $list) (i32.const 16)))
                (i32.load (i32.add (local.get $list) (i32.const 20)))))))
          (else
            (call $one_decoration (i32.const 220) (i32.const 3))
            (call $deco_set (local.get $b) (i32.const 1000) (i32.const 1) (i32.const 40))
            ;; result<_, string>: 1 = err, message at 44/48
            (if (i32.load8_u (i32.const 40))
              (then (call $log (i32.const 3) (i32.load (i32.const 44)) (i32.load (i32.const 48)))))))
        (call $drop (local.get $b))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.1#on-event") (param i32 i32))
      (func (export "forge:editor/handler@0.1.1#on-answer") (param i32 i32 i32 i32))
    )
