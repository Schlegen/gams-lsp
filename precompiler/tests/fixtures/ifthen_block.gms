$set scenario base

$ifthen %scenario%==base
* Baseline parameters
Parameters
    cap / 100 /
    cost / 10 /;
$elseif %scenario%==high
* High-cost scenario
Parameters
    cap / 80 /
    cost / 20 /;
$else
* Default fallback
Parameters
    cap / 50 /
    cost / 15 /;
$endif

Variables z;
Equations obj;
obj.. z =e= cost * cap;
