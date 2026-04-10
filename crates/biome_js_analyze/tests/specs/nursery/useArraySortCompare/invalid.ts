const anyArray: any[] = [];
anyArray.sort();
anyArray.toSorted();

anyArray.sort(undefined);
anyArray.toSorted(undefined);

const stringArray: string[] = [];
stringArray.sort();
stringArray.toSorted();

function sortReq(x: Required<{arr?: number[]}>) { x.arr.sort(); }
function sortRo(x: Readonly<{arr: number[]}>) { x.arr.sort(); }
