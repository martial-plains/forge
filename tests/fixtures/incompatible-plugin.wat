;; A component that imports `flush`, a forge:editor/log function this
;; Forge does not provide. Its imports are checked statically; neither
;; init nor any other plugin code runs.
(module
  (import "forge:editor/log@0.1.12" "log" (func $log (param i32 i32 i32)))
  (import "forge:editor/log@0.1.12" "flush" (func $flush))

  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $p i32)
    (local.set $p (i32.and
      (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
      (i32.xor (i32.sub (local.get 2) (i32.const 1)) (i32.const -1))))
    (global.set $heap (i32.add (local.get $p) (local.get 3)))
    (local.get $p))

  (func (export "forge:editor/handler@0.1.12#run-command")
    (param i32 i32 i32 i64) (result i32)
    (i32.store8 (i32.const 0) (i32.const 0)) (i32.const 0))
  (func (export "forge:editor/handler@0.1.12#on-event") (param i32 i32))
  (func (export "forge:editor/handler@0.1.12#on-answer") (param i32 i32 i32 i32))

  (func (export "init") (result i32)
    (call $flush)
    ;; result<_, string>: tag 0 = ok, in the return area at 0
    (i32.store8 (i32.const 0) (i32.const 0))
    (i32.const 0)))
