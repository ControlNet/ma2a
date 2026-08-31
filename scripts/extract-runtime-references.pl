#!/usr/bin/env perl
use strict;
use warnings;

my ($kind, $path) = @ARGV;
die "usage: $0 KIND PATH\n" unless defined $kind && defined $path;

open my $handle, '<:raw', $path or die "open $path: $!\n";
local $/;
my $source = <$handle>;
close $handle or die "close $path: $!\n";

sub emit {
    my ($reference_kind, $reference) = @_;
    $reference =~ s/^\s+|\s+$//g;
    print "$reference_kind\t$reference\n" if length $reference;
}

if ($kind eq 'html' || $kind eq 'svg') {
    while ($source =~ /(?:src|href|poster|data)\s*=\s*(["'])([^"']*)\1/gis) {
        emit('asset', $2);
    }
    while ($source =~ /srcset\s*=\s*(["'])([^"']*)\1/gis) {
        for my $candidate (split /,/, $2) {
            my ($reference) = $candidate =~ /^\s*(\S+)/;
            emit('asset', $reference) if defined $reference;
        }
    }
} elsif ($kind eq 'css') {
    while ($source =~ /url\(\s*(["']?)([^"']*)\1\s*\)/gis) {
        emit('asset', $2);
    }
    while ($source =~ /\@import\s+(["'])([^"']*)\1/gis) {
        emit('asset', $2);
    }
} elsif ($kind eq 'js') {
    while ($source =~ /(?:import\s*\(|from\s+|import\s+)(["'`])([^"'`]*)\1/gs) {
        emit('asset', $2);
    }
    while ($source =~ /new\s+URL\(\s*(["'`])([^"'`]*)\1\s*,\s*import\.meta\.url/gs) {
        emit('asset', $2);
    }
    while ($source =~ /new\s+URL\(\s*(["'`])([^"'`]*)\1\s*,\s*window\.location\.origin/gs) {
        emit('network', $2);
    }
    while ($source =~ /(?:fetch|new\s+(?:WebSocket|EventSource|Worker|SharedWorker)|navigator\.sendBeacon)\s*\(\s*(["'`])([^"'`]*)\1/gs) {
        emit('network', $2);
    }
    while ($source =~ /\.open\(\s*(["'`])(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\1\s*,\s*(["'`])([^"'`]*)\2/gis) {
        emit('network', $3);
    }
} else {
    die "unknown reference kind: $kind\n";
}
