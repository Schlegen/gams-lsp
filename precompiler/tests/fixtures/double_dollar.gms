* double_dollar.gms — $$ lets a directive appear outside column 1.
* Without $$, GAMS only recognises $ in the first column.
* Every directive below uses $$ with leading whitespace — the whole point.

    $$set scenario base
  $$setglobal model_version 3

    $$ifthen %scenario%==base
* Baseline parameters
Parameters cap / 100 /;
    $$else
Parameters cap / 80 /;
    $$endif

* Nested set-check with $$ and indentation
    $$ifthen.check set scenario
      $$set datafile data_%scenario%.gms
    $$endif.check

Sets i / 1*3 /;
Variables x(i);
