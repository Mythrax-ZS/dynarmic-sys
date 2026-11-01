#include "dynarmic/frontend/A64/translate/impl/impl.h"

namespace Dynarmic::A64 {

namespace {

constexpr u64 tag_clear_mask = 0xF0FF'FFFF'FFFF'FFFFULL;

IR::U64 AddressRegister(TranslatorVisitor& v, Reg reg) {
    return reg == Reg::SP ? IR::U64{v.SP(64)} : IR::U64{v.X(64, reg)};
}

void SetAddressRegister(TranslatorVisitor& v, Reg reg, const IR::U64& value) {
    if (reg == Reg::SP) {
        v.SP(64, value);
    } else {
        v.X(64, reg, value);
    }
}

IR::U64 WithoutTag(TranslatorVisitor& v, const IR::U64& value) {
    return v.ir.And(value, v.ir.Imm64(tag_clear_mask));
}

IR::U64 SignExtend56(TranslatorVisitor& v, const IR::U64& value) {
    return v.ir.ArithmeticShiftRight(v.ir.LogicalShiftLeft(value, v.ir.Imm8(8)), v.ir.Imm8(8));
}

IR::U64 PointerDifference(TranslatorVisitor& v, Reg Rm, Reg Rn) {
    return v.ir.Sub(SignExtend56(v, AddressRegister(v, Rn)), SignExtend56(v, AddressRegister(v, Rm)));
}

bool AddTagged(TranslatorVisitor& v, bool subtract, Imm<6> offset_imm, Reg Rn, Reg Rd) {
    const IR::U64 base = AddressRegister(v, Rn);
    const IR::U64 offset = v.ir.Imm64(offset_imm.ZeroExtend<u64>() << 4);
    const IR::U64 moved = subtract ? IR::U64{v.ir.Sub(base, offset)} : IR::U64{v.ir.Add(base, offset)};
    SetAddressRegister(v, Rd, WithoutTag(v, moved));
    return true;
}

bool StorePairWithTag(TranslatorVisitor& v, Imm<7> offset_imm, Reg Rt2, Reg Rn, Reg Rt, bool postindex, bool writeback) {
    const IR::U64 first = v.X(64, Rt);
    const IR::U64 second = v.X(64, Rt2);
    const IR::U64 base = AddressRegister(v, Rn);
    const s64 offset = concatenate(offset_imm, Imm<4>{0}).SignExtend<s64>();
    const IR::U64 offset_address = v.ir.Add(base, v.ir.Imm64(static_cast<u64>(offset)));
    const IR::U64 address = postindex ? base : offset_address;
    v.Mem(address, 8, IR::AccType::NORMAL, first);
    v.Mem(v.ir.Add(address, v.ir.Imm64(8)), 8, IR::AccType::NORMAL, second);
    if (writeback) {
        SetAddressRegister(v, Rn, offset_address);
    }
    return true;
}

}

bool TranslatorVisitor::ADDG(Imm<6> offset_imm, Imm<4>, Reg Rn, Reg Rd) {
    return AddTagged(*this, false, offset_imm, Rn, Rd);
}

bool TranslatorVisitor::SUBG(Imm<6> offset_imm, Imm<4>, Reg Rn, Reg Rd) {
    return AddTagged(*this, true, offset_imm, Rn, Rd);
}

bool TranslatorVisitor::IRG(Reg, Reg Rn, Reg Rd) {
    SetAddressRegister(*this, Rd, WithoutTag(*this, AddressRegister(*this, Rn)));
    return true;
}

bool TranslatorVisitor::GMI(Reg Rm, Reg Rn, Reg Rd) {
    const IR::U64 tag = ir.And(ir.LogicalShiftRight(AddressRegister(*this, Rn), ir.Imm8(56)), ir.Imm64(0xF));
    X(64, Rd, ir.Or(X(64, Rm), ir.LogicalShiftLeftMasked(ir.Imm64(1), tag)));
    return true;
}

bool TranslatorVisitor::SUBP(Reg Rm, Reg Rn, Reg Rd) {
    X(64, Rd, PointerDifference(*this, Rm, Rn));
    return true;
}

bool TranslatorVisitor::SUBPS(Reg Rm, Reg Rn, Reg Rd) {
    const IR::U64 result = PointerDifference(*this, Rm, Rn);
    ir.SetNZCV(ir.NZCVFrom(result));
    X(64, Rd, result);
    return true;
}

bool TranslatorVisitor::MemoryTagStore(Imm<2> opc, Imm<9> imm9, Imm<2> op2, Reg Rn, Reg Rt) {
    const auto kind = opc.ZeroExtend();
    const auto mode = op2.ZeroExtend();
    if (mode == 0b00) {
        if (kind != 0b01) {
            return UnallocatedEncoding();
        }
        X(64, Rt, WithoutTag(*this, X(64, Rt)));
        return true;
    }
    const IR::U64 base = AddressRegister(*this, Rn);
    const s64 offset = concatenate(imm9, Imm<4>{0}).SignExtend<s64>();
    const IR::U64 offset_address = ir.Add(base, ir.Imm64(static_cast<u64>(offset)));
    const IR::U64 address = mode == 0b01 ? base : offset_address;
    const size_t zeroed = kind == 0b01 ? 16 : kind == 0b11 ? 32 : 0;
    for (size_t at = 0; at < zeroed; at += 8) {
        Mem(ir.Add(address, ir.Imm64(at)), 8, IR::AccType::NORMAL, ir.Imm64(0));
    }
    if (mode != 0b10) {
        SetAddressRegister(*this, Rn, offset_address);
    }
    return true;
}

bool TranslatorVisitor::STGP_1(Imm<7> offset_imm, Reg Rt2, Reg Rn, Reg Rt) {
    return StorePairWithTag(*this, offset_imm, Rt2, Rn, Rt, true, true);
}

bool TranslatorVisitor::STGP_2(Imm<7> offset_imm, Reg Rt2, Reg Rn, Reg Rt) {
    return StorePairWithTag(*this, offset_imm, Rt2, Rn, Rt, false, true);
}

bool TranslatorVisitor::STGP_3(Imm<7> offset_imm, Reg Rt2, Reg Rn, Reg Rt) {
    return StorePairWithTag(*this, offset_imm, Rt2, Rn, Rt, false, false);
}

}
