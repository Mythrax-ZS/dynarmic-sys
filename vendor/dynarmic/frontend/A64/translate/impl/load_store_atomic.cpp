#include "dynarmic/frontend/A64/translate/impl/impl.h"

namespace Dynarmic::A64 {

namespace {

IR::U64 BaseAddress(TranslatorVisitor& v, Reg Rn) {
    return Rn == Reg::SP ? IR::U64{v.SP(64)} : IR::U64{v.X(64, Rn)};
}

IR::AccType AtomicAccType(bool acquire, bool release) {
    return acquire || release ? IR::AccType::ORDERED : IR::AccType::ATOMIC;
}

IR::UAny Element(TranslatorVisitor& v, const IR::U32U64& value, size_t bytes) {
    switch (bytes) {
    case 1:
        return v.ir.LeastSignificantByte(value);
    case 2:
        return v.ir.LeastSignificantHalf(value);
    default:
        return value;
    }
}

IR::U32U64 Extended(TranslatorVisitor& v, const IR::U32U64& value, size_t bytes, bool is_signed) {
    if (bytes > 2) {
        return value;
    }
    const IR::UAny element = Element(v, value, bytes);
    return is_signed ? v.SignExtend(element, 32) : v.ZeroExtend(element, 32);
}

IR::U64 ToLong(TranslatorVisitor& v, const IR::U32U64& value, size_t regsize) {
    return regsize == 64 ? IR::U64{value} : v.ir.ZeroExtendWordToLong(IR::U32{value});
}

IR::U32U64 EqualMask(TranslatorVisitor& v, const IR::U32U64& a, const IR::U32U64& b, size_t bitsize) {
    const IR::U32U64 difference = v.ir.Eor(a, b);
    const IR::U32U64 zero = bitsize == 64 ? IR::U32U64{v.ir.Imm64(0)} : IR::U32U64{v.ir.Imm32(0)};
    const IR::U32U64 spread = v.ir.Or(difference, v.ir.Sub(zero, difference));
    return v.ir.Not(v.ir.ArithmeticShiftRight(spread, v.ir.Imm8(static_cast<u8>(bitsize - 1))));
}

IR::U32U64 Select(TranslatorVisitor& v, const IR::U32U64& mask, const IR::U32U64& when_set, const IR::U32U64& otherwise) {
    return v.ir.Or(v.ir.And(when_set, mask), v.ir.AndNot(otherwise, mask));
}

IR::U64 StoredMask(TranslatorVisitor& v, const IR::U32& status) {
    return v.ir.SignExtendWordToLong(IR::U32{v.ir.Sub(status, v.ir.Imm32(1))});
}

void Commit(TranslatorVisitor& v, Reg reg, const IR::U64& value, const IR::U64& stored) {
    v.X(64, reg, Select(v, stored, value, v.X(64, reg)));
}

bool RetryUnlessStored(TranslatorVisitor& v, const IR::U32& status) {
    v.ir.SetCheckBit(v.ir.IsZero(status));
    const auto next = IR::Term::LinkBlock{v.ir.current_location->AdvancePC(4)};
    const auto retry = IR::Term::LinkBlock{*v.ir.current_location};
    v.ir.SetTerm(IR::Term::CheckBit{next, retry});
    return false;
}

}

bool TranslatorVisitor::LDAPR(Imm<2> size, Reg Rn, Reg Rt) {
    return LDAR(size, Rn, Rt);
}

bool TranslatorVisitor::AtomicMemoryOperation(Imm<2> size, bool A, bool R, Reg Rs, Imm<4> opc, Reg Rn, Reg Rt) {
    const auto op = opc.ZeroExtend();
    if (op > 0b1000) {
        return UnallocatedEncoding();
    }
    const size_t bytes = size_t{1} << size.ZeroExtend<size_t>();
    const size_t regsize = bytes == 8 ? 64 : 32;
    const auto acctype = AtomicAccType(A, R);
    const IR::U64 address = BaseAddress(*this, Rn);
    const IR::U32U64 operand = X(regsize, Rs);
    const IR::U32U64 old = ZeroExtend(ExclusiveMem(address, bytes, acctype), regsize);

    const IR::U32U64 updated = [&]() -> IR::U32U64 {
        switch (op) {
        case 0b0000:
            return ir.Add(old, operand);
        case 0b0001:
            return ir.AndNot(old, operand);
        case 0b0010:
            return ir.Eor(old, operand);
        case 0b0011:
            return ir.Or(old, operand);
        case 0b0100:
            return ir.MaxSigned(Extended(*this, old, bytes, true), Extended(*this, operand, bytes, true));
        case 0b0101:
            return ir.MinSigned(Extended(*this, old, bytes, true), Extended(*this, operand, bytes, true));
        case 0b0110:
            return ir.MaxUnsigned(Extended(*this, old, bytes, false), Extended(*this, operand, bytes, false));
        case 0b0111:
            return ir.MinUnsigned(Extended(*this, old, bytes, false), Extended(*this, operand, bytes, false));
        default:
            return operand;
        }
    }();

    const IR::U32 status = ExclusiveMem(address, bytes, acctype, Element(*this, updated, bytes));
    Commit(*this, Rt, ToLong(*this, old, regsize), StoredMask(*this, status));
    return RetryUnlessStored(*this, status);
}

bool TranslatorVisitor::CAS(Imm<2> size, bool L, Reg Rs, bool o0, Reg Rn, Reg Rt) {
    const size_t bytes = size_t{1} << size.ZeroExtend<size_t>();
    const size_t regsize = bytes == 8 ? 64 : 32;
    const auto acctype = AtomicAccType(L, o0);
    const IR::U64 address = BaseAddress(*this, Rn);
    const IR::U32U64 expected = Extended(*this, X(regsize, Rs), bytes, false);
    const IR::U32U64 desired = X(regsize, Rt);
    const IR::U32U64 old = ZeroExtend(ExclusiveMem(address, bytes, acctype), regsize);
    const IR::U32U64 stored = Select(*this, EqualMask(*this, old, expected, regsize), desired, old);
    const IR::U32 status = ExclusiveMem(address, bytes, acctype, Element(*this, stored, bytes));
    Commit(*this, Rs, ToLong(*this, old, regsize), StoredMask(*this, status));
    return RetryUnlessStored(*this, status);
}

bool TranslatorVisitor::CASP(bool sz, bool L, Reg Rs, bool o0, Reg Rn, Reg Rt) {
    if ((static_cast<size_t>(Rs) & 1) != 0 || (static_cast<size_t>(Rt) & 1) != 0) {
        return UnallocatedEncoding();
    }
    const auto acctype = AtomicAccType(L, o0);
    const IR::U64 address = BaseAddress(*this, Rn);

    if (!sz) {
        const IR::U64 expected = ir.Pack2x32To1x64(X(32, Rs), X(32, Rs + 1));
        const IR::U64 desired = ir.Pack2x32To1x64(X(32, Rt), X(32, Rt + 1));
        const IR::U64 old = ExclusiveMem(address, 8, acctype);
        const IR::U32U64 stored = Select(*this, EqualMask(*this, old, expected, 64), desired, old);
        const IR::U32 status = ExclusiveMem(address, 8, acctype, stored);
        const IR::U64 mask = StoredMask(*this, status);
        Commit(*this, Rs, ir.ZeroExtendWordToLong(ir.LeastSignificantWord(old)), mask);
        Commit(*this, Rs + 1, ir.ZeroExtendWordToLong(ir.MostSignificantWord(old).result), mask);
        return RetryUnlessStored(*this, status);
    }

    const IR::U64 expected_low = X(64, Rs);
    const IR::U64 expected_high = X(64, Rs + 1);
    const IR::U64 desired_low = X(64, Rt);
    const IR::U64 desired_high = X(64, Rt + 1);
    const IR::U128 old = ExclusiveMem(address, 16, acctype);
    const IR::U64 old_low = ir.VectorGetElement(64, old, 0);
    const IR::U64 old_high = ir.VectorGetElement(64, old, 1);
    const IR::U32U64 equal = ir.And(EqualMask(*this, old_low, expected_low, 64), EqualMask(*this, old_high, expected_high, 64));
    const IR::U128 stored = ir.Pack2x64To1x128(Select(*this, equal, desired_low, old_low), Select(*this, equal, desired_high, old_high));
    const IR::U32 status = ExclusiveMem(address, 16, acctype, stored);
    const IR::U64 mask = StoredMask(*this, status);
    Commit(*this, Rs, old_low, mask);
    Commit(*this, Rs + 1, old_high, mask);
    return RetryUnlessStored(*this, status);
}

}
