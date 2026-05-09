Sets
    i  'supply nodes'  / s1, s2 /
    j  'demand nodes'  / d1, d2, d3 /;

Parameters
    supply(i)  / s1 100, s2 200 /
    demand(j)  / d1 80, d2 120, d3 100 /;

Variables
    x(i,j)  'shipment quantity'
    cost     'total cost';

Positive Variable x;
