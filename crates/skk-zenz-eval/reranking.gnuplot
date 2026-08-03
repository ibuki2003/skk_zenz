if (!exists("datafile")) datafile = "reranking.tsv"
if (!exists("outputfile")) outputfile = "reranking.svg"
if (!exists("interactive")) interactive = 0
if (!exists("metric")) metric = 3

array labels[10] = [ \
    "prob: none", \
    "prob: correction", \
    "prob: sigmoid", \
    "prob: entropy", \
    "prob: sigmoid + correction", \
    "prob: entropy + correction", \
    "log prob: smooth correction", \
    "log prob: smooth fixed", \
    "log prob: smooth sigmoid", \
    "log prob: smooth entropy" \
]

set xlabel "lambda (AI weight)"
set xrange [0:1]
set grid back
set key outside right center box noenhanced font "sans,10" samplen 2 spacing 0.9 maxrows 12

if (interactive) {
    set mouse

    metric_title = metric == 4 ? "Top-1 rate" : "MRR"
    set terminal qt title metric_title size 1600,900 enhanced font "sans,12" replotonresize
    set title metric_title
    plot for [i=0:9] datafile index i using 2:(column(metric)) with lines title labels[i + 1]

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
    plot for [i=0:9] datafile index i using 2:3 with lines title labels[i + 1]

    set title "Top-1 rate"
    plot for [i=0:9] datafile index i using 2:4 with lines title labels[i + 1]

    unset multiplot
}
