* Tagged $ifthen blocks — the tag matches $elseif/$endif to the opening $ifthen.
* This is idiomatic GAMS for nested or named conditional sections.

$set scenario base

$ifthen.scen %scenario%==base
* Baseline parameters
Parameters
    cap / 100 /
    cost / 10 /;
$elseif.scen %scenario%==high
* High-cost scenario
Parameters
    cap / 80 /
    cost / 20 /;
$else.scen
* Unknown scenario — use defaults
Parameters
    cap / 60 /
    cost / 12 /;
$endif.scen

* Nested tagged blocks
$ifthen.outer %scenario%==base

$ifthen.inner set scenario
* scenario variable is defined
    Variables z;
$endif.inner

    $$ifthen.inner set scenario
    * scenario variable is defined
        Variables a;
    $$endif.inner

$endif.outer

Variables totalCost;
Equations objDef;
objDef.. totalCost =e= cost * cap;
