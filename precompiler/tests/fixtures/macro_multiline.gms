* Macro definitions for cost calculation

$macro calcCost(q, p) \
q * p

$macro myxor(a,b)  (a or b) \ 
    and (not a or not b)

$macro sumOver(expr, idx) \
sum(idx, expr)

* Single-line macro
$macro double(x) 2 * x

Parameters
    qty  / 50 /
    price / 3 /;

Variables totalCost;
Equations objDef;

objDef.. totalCost =e= calcCost(qty, price);
