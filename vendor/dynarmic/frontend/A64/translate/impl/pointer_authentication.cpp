#include "dynarmic/frontend/A64/translate/impl/impl.h"

namespace Dynarmic::A64 {

namespace {

bool PointerAuthenticationIdentity(TranslatorVisitor& v, bool Z, Reg Rn) {
    if (Z && Rn != Reg::R31) {
        return v.UnallocatedEncoding();
    }
    return true;
}

}

bool TranslatorVisitor::PACIA_1(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::PACIB_1(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::AUTIA_1(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::AUTIB_1(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::PACDA(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::PACDB(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::AUTDA(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::AUTDB(bool Z, Reg Rn, Reg) {
    return PointerAuthenticationIdentity(*this, Z, Rn);
}

bool TranslatorVisitor::XPAC_1(bool, Reg) {
    return true;
}

bool TranslatorVisitor::PACGA(Reg Rm, Reg Rn, Reg Rd) {
    const IR::U64 modifier = Rm == Reg::SP ? IR::U64{SP(64)} : IR::U64{X(64, Rm)};
    X(64, Rd, ir.And(ir.Eor(X(64, Rn), modifier), ir.Imm64(0xFFFF'FFFF'0000'0000ULL)));
    return true;
}

bool TranslatorVisitor::BRA(bool Z, bool, Reg Rn, Reg Rm) {
    if (!Z && Rm != Reg::R31) {
        return UnallocatedEncoding();
    }
    return BR(Rn);
}

bool TranslatorVisitor::BLRA(bool Z, bool, Reg Rn, Reg Rm) {
    if (!Z && Rm != Reg::R31) {
        return UnallocatedEncoding();
    }
    return BLR(Rn);
}

bool TranslatorVisitor::RETA(bool) {
    return RET(Reg::R30);
}

bool TranslatorVisitor::LDRA(bool, bool S, Imm<9> imm9, bool W, Reg Rn, Reg Rt) {
    const s64 offset = concatenate(Imm<1>{S ? 1u : 0u}, imm9, Imm<3>{0}).SignExtend<s64>();
    const IR::U64 base = Rn == Reg::SP ? IR::U64{SP(64)} : IR::U64{X(64, Rn)};
    const IR::U64 address = ir.Add(base, ir.Imm64(static_cast<u64>(offset)));
    const IR::U64 data = Mem(address, 8, IR::AccType::NORMAL);
    if (W) {
        if (Rn == Reg::SP) {
            SP(64, address);
        } else {
            X(64, Rn, address);
        }
    }
    X(64, Rt, data);
    return true;
}

}
