function stringWiden(b: boolean): string { if (b) return "loading"; return "idle"; }
function numberWiden(b: boolean): number { if (b) return 200; return 404; }
function bigintWiden(b: boolean): bigint { if (b) return 1n; return 2n; }

const asConstArrow = (): string => "main" as const;
const asConstParen = (): string => ("north" as const);
const asConstNumber = (): number => 42 as const;
const asConstBlock = (): string => { return "only" as const; };

const arrowMulti = (): string => { if (Math.random() > 0.5) return "yes"; return "no"; };

const fnExprMulti = function(b: boolean): string { if (b) return "dark"; return "light"; };

function nestedPrune(): string {
    function inner(): number { return 42; }
    if (Math.random() > 0.5) return "a";
    return "b";
}

async function asyncMulti(b: boolean): Promise<string> { if (b) return "a"; return "b"; }

function unionSubset(b: boolean): "a" | "b" | "c" { if (b) return "a"; return "b"; }

function objectAsConst(): { a: string; b: string } {
    return { a: "x", b: "y" } as const;
}

async function asyncAsConst(): Promise<string> { return "hello" as const; }

function unwrap<T>(x: T): T | null { return x; }

function asConstVar(): string {
    const result = "hello" as const;
    return result;
}

function recordWider(): Record<string, string> {
    return { a: "x", b: "y" } as const;
}

function nullable(flag: boolean): string | null { if (flag) return "yes"; return null; }

const satisfiesConst = (): string => ("idle" as const) satisfies string;

function angleBracketConst(): string { return <const>"hello"; }

function tupleWiden(): [string, number] { return ["hello", 42] as const; }

function ternaryWiden(b: boolean): string { return b ? "a" : "b"; }

function pickWider(): Pick<{name: string, age: number}, "name"> { return {name: "hello"} as const; }

function omitWider(): Omit<{name: string, age: number}, "age"> { return {name: "hello"} as const; }

function readonlyWider(): Readonly<{name: string}> { return {name: "hello"} as const; }

class StatusClass { getStatus(b: boolean): string { if (b) return "loading"; return "idle"; } }

const modeObj = { getMode(b: boolean): string { if (b) return "dark"; return "light"; } };

class CodeClass { getCode(b: boolean): number { if (b) return 200; return 404; } }

const codeObj = { getCode(b: boolean): number { if (b) return 200; return 404; } };

class GetterClass { get code(): number { if (Math.random() > 0.5) return 200; return 404; } }

const getterObj = { get code(): number { if (Math.random() > 0.5) return 200; return 404; } };

class AsyncMethod { async getStatus(b: boolean): Promise<string> { if (b) return "loading"; return "idle"; } }

function singleWithNull(): string | null { return "hello"; }
function singleWithExtraPrimitive(): string | number { return "hello"; }
function singleBoolWithNull(): boolean | null { return true; }
function singleWithExtraLiteral(): string | 0 { return "hello"; }
function threeVariantsSingleReturn(): string | number | null { return "hello"; }
function partialReduce(b: boolean): string | number | null { if (b) return "a"; return 1; }
function multiLiteralNarrow(b: boolean): string | null { if (b) return "a"; return "b"; }
function exactMatchDropNull(b: boolean): "a" | "b" | null { if (b) return "a"; return "b"; }
function asConstUnionAnnotation(): string | null { return "hello" as const; }

async function asyncUnionNull(): Promise<string | null> { return "hello"; }

const arrowUnionNull = (): string | null => "hello";

class UnionClass { getValue(): string | null { return "hello"; } }

const unionObj = { getValue(): string | null { return "hello"; } };

class UnionGetter { get value(): string | null { return "hello"; } }

function nestedUnion(): string | (number | null) { return "hello"; }

type SimpleAlias = string | null;
function simpleAliasUnion(): SimpleAlias { return "hello"; }

function throwBranchUnion(b: boolean): string | null {
    if (b) throw new Error("fail");
    return "hello";
}

async function asyncUnionBothReturns(b: boolean): Promise<string | null> {
    if (b) return "hello";
    return null;
}

function partialAbsorbUnion(b: boolean): "a" | "b" | string | null { if (b) return "a"; return null; }

function crossPrimitiveUnion(): "a" | string | 1 { return "a"; }
