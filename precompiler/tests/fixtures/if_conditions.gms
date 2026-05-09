* if_conditions.gms
* Examples from GAMS documentation showing $if (single-line) with various
* condition types: set, not set, exist, not exist, errorlevel, setEnv.

$set input file.gms
$set output file_out.gms

* Check if a variable is set
$if set input $log input is set to "%input%"

* Check if a variable is NOT set
$if not set missing_var $log missing_var is not defined

* File existence check
$if not exist nonexistent_file.gms $log file not found

* Chain of $if guards (from GAMS docs decompress example)
$if not set input  $set input file_c.gms
$if not set output $set output file.gms
$log Processing %input% into %output%

* setGlobal / setLocal variants
$setglobal gvar global_value
$if setGlobal gvar $log global var is set

* errorlevel check (after external call)
$if errorlevel 1 $abort Non-zero error level detected

Sets i / 1*3 /;
