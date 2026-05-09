* Transport model — mixed directives example
* Author: test fixture

$set scenario base
$setglobal model_version 2

$ifthen.set scenario
* Scenario is defined — load scenario-specific data
$set datafile data_%scenario%.gms
$else
$set datafile data_default.gms
$endif

$ontext
This block is a multi-line comment.
It can contain anything: $set, $include, etc.
All ignored by the preprocessor.
$offtext

Sets
    i  'supply nodes'  / s1, s2 /
    j  'demand nodes'  / d1, d2 /;

* End of model
