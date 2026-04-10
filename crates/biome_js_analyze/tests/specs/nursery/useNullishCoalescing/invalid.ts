// should generate diagnostics

// Nullish literals
const a: null = null;
const b = a || 'default';

const c: undefined = undefined;
const d = c || 'fallback';

// Union with null
declare const maybeString: string | null;
const e = maybeString || 'default';

// Union with undefined
declare const maybeNumber: number | undefined;
const f = maybeNumber || 0;

// Union with null and undefined
declare const maybeValue: string | null | undefined;
const g = maybeValue || 'fallback';

// Function return
function getValue(x: string | null): string {
  return x || 'default';
}

// Arrow function
const getDefault = (x: number | undefined) => x || 42;

// Object property
declare const obj: { prop: string | null };
const h = obj.prop || 'default';

// Array element
declare const arr: (number | null)[];
const i = arr[0] || 0;

// Nested in parentheses
declare const val: string | null;
const j = (val || 'default').toUpperCase();

// Assignment
let result: string;
declare const source: string | null;
result = source || 'fallback';

// Chained ||
declare const x: string | null;
declare const y: string | null;
const k = x || y || 'default';

// Mixed nullish + falsy types (no safe fix)
declare const mixedValue: number | null;
const unsafeFix = mixedValue || 100;

// Optional property in type alias
type TypeWithOptional = { a?: string; b: number };
declare const objWithOptional: TypeWithOptional;
const optionalFromType = objWithOptional.a || 'default';

// Optional property in interface
interface InterfaceWithOptional {
  a?: string;
  b: number;
}
declare const objFromInterface: InterfaceWithOptional;
const optionalFromInterface = objFromInterface.a || 'default';

// Optional property in Pick
declare const pickOptional: Pick<{a?: string, b: number}, "a">;
const vPickOpt = pickOptional.a || "default";

// Nullable property in Pick
declare const pickNullable: Pick<{a: string | null}, "a">;
const vPickNull = pickNullable.a || "default";

// Nullable property in Omit
declare const omitNullable: Omit<{x: number, y: string | null}, "x">;
const vOmitNull = omitNullable.y || "default";

// Nested nullable object in Pick
declare const nestPick: Pick<{cfg: {name: string} | null}, "cfg">;
const nestV = nestPick.cfg || {name: "d"};

// Optional property in Omit
declare const omitOpt: Omit<{a: string, b?: number}, "a">;
const omitV = omitOpt.b || 0;
