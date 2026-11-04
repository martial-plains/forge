;; A test plugin for config.get (forge:editor 0.1.12). Command:
;;   config-run: log (info) whatever the user gave this plugin with
;;     forge.plugin.config.
(module
      (import "forge:editor/log@0.1.19" "log" (func $log (param i32 i32 i32)))
      (import "forge:editor/commands@0.1.19" "register" (func $register (param i32 i32 i32 i32 i32)))
      (import "forge:editor/config@0.1.19" "get" (func $get (param i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "config-run")          ;; 10
      (data (i32.const 140) "Show the config.")     ;; 16

      (global $heap (mut i32) (i32.const 4096))
      (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p))

      (func (export "init") (result i32)
        (call $register (i32.const 100) (i32.const 10) (i32.const 140) (i32.const 16) (i32.const 16))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.19#run-command")
        (param $name i32) (param $len i32) (param i32 i64) (result i32)
        ;; config.get() -> string at 40 (pointer, length)
        (call $get (i32.const 40))
        (call $log (i32.const 1) (i32.load (i32.const 40)) (i32.load (i32.const 44)))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.19#on-event") (param i32 i32))
      (func (export "forge:editor/handler@0.1.19#on-answer") (param i32 i32 i32 i32))
    )
