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

// Partial type
declare const partialObj: Partial<{name: string}>;
const partialOr = partialObj.name || 'default';

type P = Partial<{a: number}>;
declare const p: P;
const v = p.a || 0;

// Partial<T> with nullable base member
declare const partNull: Partial<{a: string | null}>;
const vPartNull = partNull.a || "default";

// Required with nullable optional
declare const reqNull: Required<{a?: string | null}>;
const vReqNull = reqNull.a || "default";

// Readonly with optional property
declare const roOpt: Readonly<{a?: string}>;
const vRoOpt = roOpt.a || "default";
