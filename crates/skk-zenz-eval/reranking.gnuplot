if (!exists("datafile")) datafile = "reranking.tsv"
if (!exists("outputfile")) outputfile = "reranking.svg"
if (!exists("interactive")) interactive = 0
if (!exists("metric")) metric = 3

stats datafile every ::1 nooutput
strategy_count = (STATS_columns - 1) / 2

set xlabel "lambda (AI weight)"
set xrange [0:1]
set grid back
set key outside right center box noenhanced font "sans,10" samplen 2 spacing 0.9 maxrows 12

if (interactive) {
    set mouse

    metric_title = metric == 4 ? "Top-1 rate" : "MRR"
    set terminal qt title metric_title size 1600,900 enhanced font "sans,12" replotonresize
    set title metric_title
    metric_offset = metric == 4 ? 3 : 2
    plot for [i=0:strategy_count-1] datafile using 1:(column(metric_offset + 2*i)) \
        with lines title columnhead(metric_offset + 2*i)

    print "Interactive controls:"
    print "  click a legend entry: toggle the series"
    print "  left drag: zoom; u: unzoom; p/n: zoom history"
    print "  close the window to exit"
    pause mouse close
} else {
    set terminal svg size 2400,1400 dynamic enhanced font "sans,18"
    set output outputfile
    set multiplot layout 2,1 title "Reranking strategies"

    set title "MRR"
    plot for [i=0:strategy_count-1] datafile using 1:(column(2 + 2*i)) \
        with lines title columnhead(2 + 2*i)

    set title "Top-1 rate"
    plot for [i=0:strategy_count-1] datafile using 1:(column(3 + 2*i)) \
        with lines title columnhead(3 + 2*i)

    unset multiplot
}
