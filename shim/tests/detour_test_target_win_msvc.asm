; Known six-byte prologue for the Windows detour end-to-end test.
; Keep this byte shape aligned with detour_reloc_test.cpp:
;   push rbp; mov rbp,rsp; push r15

.code

PUBLIC s2_detour_test_target
s2_detour_test_target PROC
    push rbp
    mov rbp, rsp
    push r15
    mov rax, rcx
    add rax, 1
    pop r15
    pop rbp
    ret
s2_detour_test_target ENDP

END
