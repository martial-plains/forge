;; A test plugin for region maps (forge:editor 0.1.9): a keymap a stretch
;; of text brings with it, and clicks as keys. Written against the
;; canonical ABI like the other fixtures. Commands:
;;   region-run: bind "<mouse-1>" to kill-line in the region map
;;     "test-word", attach that map to line 0 columns 0-5 of the current
;;     buffer, then log "bound".
;;   region-oops: bind a key that isn't valid notation, log the error.
(module
      (import "forge:editor/log@0.1.19" "log" (func $log (param i32 i32 i32)))
      (import "forge:editor/commands@0.1.19" "register" (func $register (param i32 i32 i32 i32 i32)))
      (import "forge:editor/keymap@0.1.19" "bind-region"
        (func $bind_region (param i32 i32 i32 i32 i32 i32 i32)))
      (import "forge:editor/buffers@0.1.19" "[static]buffer.current" (func $current (result i32)))
      (import "forge:editor/buffers@0.1.19" "[resource-drop]buffer" (func $drop (param i32)))
      (import "forge:editor/decorations@0.1.19" "set-regions"
        (func $set_regions (param i32 i32 i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 100) "region-run")          ;; 10
      (data (i32.const 120) "region-oops")         ;; 12
      (data (i32.const 140) "Mark the word.")      ;; 16
      (data (i32.const 160) "test-word")           ;; 9
      (data (i32.const 180) "<mouse-1>")           ;; 9
      (data (i32.const 200) "kill-line")           ;; 9
      (data (i32.const 220) "C-c nonsense")        ;; 12
      (data (i32.const 240) "bound")               ;; 5

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
        (call $register (i32.const 120) (i32.const 11) (i32.const 140) (i32.const 16) (i32.const 16))

        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.19#run-command")
        (param $name i32) (param $len i32) (param i32 i64) (result i32)
        (local $b i32)
        (if (i32.eq (local.get $len) (i32.const 10))
          (then
            ;; keymap.bind-region("test-word", "<mouse-1>", "kill-line")
            ;;   -> result at 40
            (call $bind_region
              (i32.const 160) (i32.const 9)
              (i32.const 180) (i32.const 9)
              (i32.const 200) (i32.const 9)
              (i32.const 40))
            ;; One region: span (0,0)-(0,5), keymap "test-word".
            (i32.store (i32.const 1000) (i32.const 0))   ;; start line
            (i32.store (i32.const 1004) (i32.const 0))   ;; start column
            (i32.store (i32.const 1008) (i32.const 0))   ;; end line
            (i32.store (i32.const 1012) (i32.const 5))   ;; end column
            (i32.store (i32.const 1016) (i32.const 160)) ;; keymap ptr
            (i32.store (i32.const 1020) (i32.const 9))   ;; keymap len
            (local.set $b (call $current))
            ;; decorations.set-regions(buffer, list of one) -> result at 48
            (call $set_regions (local.get $b) (i32.const 1000) (i32.const 1) (i32.const 48))
            (call $drop (local.get $b))
            (if (i32.load8_u (i32.const 40))
              (then (call $log (i32.const 3) (i32.load (i32.const 44)) (i32.load (i32.const 48))))
              (else
                (if (i32.load8_u (i32.const 48))
                  (then (call $log (i32.const 3) (i32.load (i32.const 52)) (i32.load (i32.const 56))))
                  (else (call $log (i32.const 1) (i32.const 240) (i32.const 5)))))))
          (else
            ;; A key that isn't valid notation: the error says so.
            (call $bind_region
              (i32.const 160) (i32.const 9)
              (i32.const 220) (i32.const 12)
              (i32.const 200) (i32.const 9)
              (i32.const 40))
            (if (i32.load8_u (i32.const 40))
              (then (call $log (i32.const 3) (i32.load (i32.const 44)) (i32.load (i32.const 48)))))))
        (i32.store8 (i32.const 0) (i32.const 0))
        (i32.const 0))

      (func (export "forge:editor/handler@0.1.19#on-event") (param i32 i32))
      (func (export "forge:editor/handler@0.1.19#on-answer") (param i32 i32 i32 i32))
    )