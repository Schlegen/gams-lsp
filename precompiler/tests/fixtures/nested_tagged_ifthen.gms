* nested_tagged_ifthen.gms
* Verbatim from GAMS documentation (UG_DollarControlOptions.html)
* Demonstrates nested $ifthen blocks with matching tags.

$set x hello
$set a hello
$set b hello
$set c hello

$ifThen.one x == y
display "it1";
$elseIf.one a == a
display "it2";
$ifThen.two c == c
display "it3";
$endIf.two
$elseIf.one b == b
display "it4";
$endIf.one
