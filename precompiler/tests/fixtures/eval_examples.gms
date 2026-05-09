* eval_examples.gms
* $eval and $evalGlobal — compile-time arithmetic.
* Examples from GAMS documentation (UG_DollarControlOptions.html).

* Basic $eval
$eval b1 ifthen(uniform(0,1)<0.5,0,1)
$eval b2 ifthen(uniform(0,1)<0.5,0,1)
$eval b3 (%b1%)xor(%b2%)
$log b1=%b1% b2=%b2% b1 xor b2=%b3%

* Division by zero — IEEE nonstop arithmetic, no error raised
$eval OneDividedByZero 1/0
$log 1/0=%OneDividedByZero%

* Using a GAMS scalar inside $eval
Scalar ac 'Avogadro constant' / 6.0221409e+23 /;
$eval log_ac round(log10(ac))
$log round(log10(ac))=%log_ac%
Set d / d0*d%log_ac% /;
$eval card_d card(d)
$log card(d)=%card_d%

* system.pi
$eval GAMSPi system.pi
$log GAMSPi=%GAMSPi%

* $evalGlobal — result is globally scoped
$evalGlobal version_major 4

* $eval.Set — evaluate a set attribute
Singleton Set h Greeting / Hello 'Welcome' /;
Set p Person / Mr.President 'Male'
               Mrs.Chancellor 'Female' /;

$eval.Set X h.TE
$log %X%
$eval.Set X p.lastTL
$log %X%
$eval.Set X p.FirstTN
$log %X%
