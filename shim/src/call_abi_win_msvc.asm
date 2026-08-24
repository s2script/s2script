; Exact Microsoft x64 outbound-call bridge.
;
; int S2_InvokeMicrosoftX64(void* fn, const uint64_t* slots,
;                           const uint8_t* classes, int count,
;                           int retKind, uint64_t* out);
;
; `slots` and `classes` are in author position order (including receiver at position zero when
; present). Class 0 is GP, class 1 is f32. Positions 0..3 select RCX/RDX/R8/R9 or XMM0..3; later
; positions are typed eight-byte stack slots. The fixed frame includes the mandatory 32-byte shadow
; space plus every supported stack position. No C++ function-pointer type describes the target.

_TEXT SEGMENT

S2_InvokeMicrosoftX64 PROC FRAME
    ; 32 bytes target shadow + 14*8 target stack slots + saved bridge inputs.
    ; 0C8h changes entry RSP (8 mod 16) to call-site alignment (0 mod 16).
    sub     rsp, 0C8h
    .allocstack 0C8h
    .endprolog

    mov     qword ptr [rsp+090h], rcx       ; target
    mov     qword ptr [rsp+098h], rdx       ; slots
    mov     qword ptr [rsp+0A0h], r8        ; classes
    mov     dword ptr [rsp+0A8h], r9d       ; count
    mov     eax, dword ptr [rsp+0F0h]       ; incoming arg 5: retKind
    mov     dword ptr [rsp+0B0h], eax
    mov     rax, qword ptr [rsp+0F8h]       ; incoming arg 6: out
    mov     qword ptr [rsp+0B8h], rax

    ; Position 4+ lives at caller RSP + position*8. After CALL pushes the return address this is
    ; exactly callee [RSP+40] onward. Raw f32 payloads occupy the low 32 bits of their eight-byte slot.
    cmp     r9d, 4
    jle     stack_done
    mov     r10d, 4
stack_loop:
    mov     r11, qword ptr [rsp+098h]
    mov     rax, qword ptr [r11+r10*8]
    mov     qword ptr [rsp+r10*8], rax
    inc     r10d
    cmp     r10d, dword ptr [rsp+0A8h]
    jl      stack_loop
stack_done:

    ; Position 0 -> RCX or XMM0.
    cmp     dword ptr [rsp+0A8h], 0
    jle     position1
    mov     r10, qword ptr [rsp+098h]
    mov     r11, qword ptr [rsp+0A0h]
    cmp     byte ptr [r11], 1
    je      position0_f32
    mov     rcx, qword ptr [r10]
    jmp     position1
position0_f32:
    movd    xmm0, dword ptr [r10]

    ; Position 1 -> RDX or XMM1.
position1:
    cmp     dword ptr [rsp+0A8h], 1
    jle     position2
    mov     r10, qword ptr [rsp+098h]
    mov     r11, qword ptr [rsp+0A0h]
    cmp     byte ptr [r11+1], 1
    je      position1_f32
    mov     rdx, qword ptr [r10+8]
    jmp     position2
position1_f32:
    movd    xmm1, dword ptr [r10+8]

    ; Position 2 -> R8 or XMM2.
position2:
    cmp     dword ptr [rsp+0A8h], 2
    jle     position3
    mov     r10, qword ptr [rsp+098h]
    mov     r11, qword ptr [rsp+0A0h]
    cmp     byte ptr [r11+2], 1
    je      position2_f32
    mov     r8, qword ptr [r10+16]
    jmp     position3
position2_f32:
    movd    xmm2, dword ptr [r10+16]

    ; Position 3 -> R9 or XMM3.
position3:
    cmp     dword ptr [rsp+0A8h], 3
    jle     invoke
    mov     r10, qword ptr [rsp+098h]
    mov     r11, qword ptr [rsp+0A0h]
    cmp     byte ptr [r11+3], 1
    je      position3_f32
    mov     r9, qword ptr [r10+24]
    jmp     invoke
position3_f32:
    movd    xmm3, dword ptr [r10+24]

invoke:
    call    qword ptr [rsp+090h]

    ; Integer/pointer returns arrive in RAX. A float return arrives in XMM0; MOVD zero-extends its
    ; exact IEEE-754 payload into RAX so the common raw-bit wire is identical on both return classes.
    cmp     dword ptr [rsp+0B0h], 1
    jne     store_result
    movd    eax, xmm0
store_result:
    mov     r10, qword ptr [rsp+0B8h]
    test    r10, r10
    je      success
    mov     qword ptr [r10], rax
success:
    mov     eax, 1
    add     rsp, 0C8h
    ret
S2_InvokeMicrosoftX64 ENDP

_TEXT ENDS
END
