#!/usr/bin/env perl
use strict;
use warnings;

my ($kind, $path) = @ARGV;
die "usage: $0 KIND PATH\n" unless defined $kind && defined $path;

open my $handle, '<:raw', $path or die "open $path: $!\n";
local $/;
my $source = <$handle>;
close $handle or die "close $path: $!\n";
binmode STDOUT;

# Browsers drop tabs and newlines anywhere in a URL and control characters around it, so a
# reference that still holds one after trimming is refused: it would not be checked as browsers
# read it, and it would break this tab-separated, one-line-per-reference output.
sub emit {
    my ($reference_kind, $reference) = @_;
    $reference =~ s/^\s+|\s+$//g;
    die "control character in a runtime reference in $path\n" if $reference =~ /[\x00-\x1f\x7f]/;
    print "$reference_kind\t$reference\n" if length $reference;
}

if ($kind eq 'html' || $kind eq 'svg') {
    while ($source =~ /(?:src|href|poster|data)\s*=\s*(?:"([^"]*)"|'([^']*)')/gis) {
        emit('asset', defined $1 ? $1 : $2);
    }
    while ($source =~ /srcset\s*=\s*(?:"([^"]*)"|'([^']*)')/gis) {
        my $value = defined $1 ? $1 : $2;
        for my $candidate (split /,/, $value) {
            my ($reference) = $candidate =~ /^\s*(\S+)/;
            emit('asset', $reference) if defined $reference;
        }
    }
} elsif ($kind eq 'css') {
    while ($source =~ /url\(\s*(?:"([^"]*)"|'([^']*)'|([^"')\s]+))\s*\)/gis) {
        emit('asset', defined $1 ? $1 : defined $2 ? $2 : $3);
    }
    while ($source =~ /\@import\s+(?:"([^"]*)"|'([^']*)')/gis) {
        emit('asset', defined $1 ? $1 : $2);
    }
} elsif ($kind eq 'js') {
    my @tokens = js_tokens($source);
    for my $at (0 .. $#tokens) {
        my ($type, $value) = @{ $tokens[$at] };
        next unless $type eq 'name';
        my $member = $at > 0 && $tokens[$at - 1][0] eq 'punct' && $tokens[$at - 1][1] eq '.';
        if (!$member && ($value eq 'import' || $value eq 'from') && is_string($tokens[$at + 1])) {
            emit('asset', $tokens[$at + 1][1]);
        } elsif (!$member && $value eq 'import' && is_punct($tokens[$at + 1], '(') && is_string($tokens[$at + 2])) {
            emit('asset', $tokens[$at + 2][1]);
        } elsif ($value eq 'new' && is_name($tokens[$at + 1], 'URL') && is_punct($tokens[$at + 2], '(')
            && is_string($tokens[$at + 3]) && is_punct($tokens[$at + 4], ',')) {
            my $base = join '', map { defined $_ ? $_->[1] : '' } @tokens[$at + 5 .. $at + 9];
            emit('asset', $tokens[$at + 3][1]) if $base =~ /^import\.meta\.url/;
            emit('network', $tokens[$at + 3][1]) if $base =~ /^window\.location\.origin/;
        } elsif ((!$member && $value eq 'fetch' && is_punct($tokens[$at + 1], '(') && is_string($tokens[$at + 2]))) {
            emit('network', $tokens[$at + 2][1]);
        } elsif ($value eq 'new' && defined $tokens[$at + 1]
            && $tokens[$at + 1][1] =~ /^(?:WebSocket|EventSource|Worker|SharedWorker)$/
            && is_punct($tokens[$at + 2], '(') && is_string($tokens[$at + 3])) {
            emit('network', $tokens[$at + 3][1]);
        } elsif ($member && ($value eq 'sendBeacon' || $value eq 'open') && is_punct($tokens[$at + 1], '(')
            && is_string($tokens[$at + 2])) {
            if ($value eq 'sendBeacon') {
                emit('network', $tokens[$at + 2][1]);
            } elsif ($tokens[$at + 2][1] =~ /^(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)$/i
                && is_punct($tokens[$at + 3], ',') && is_string($tokens[$at + 4])) {
                emit('network', $tokens[$at + 4][1]);
            }
        }
    }
} else {
    die "unknown reference kind: $kind\n";
}

sub is_string { my ($token) = @_; return defined $token && $token->[0] eq 'string'; }
sub is_punct { my ($token, $value) = @_; return defined $token && $token->[0] eq 'punct' && $token->[1] eq $value; }
sub is_name { my ($token, $value) = @_; return defined $token && $token->[0] eq 'name' && $token->[1] eq $value; }

# Splits JavaScript into names, punctuators, and string literals, skipping comments and regular
# expression literals, so text inside a string, template, comment, or pattern is never read as code.
# A template literal with substitutions is one string token holding its raw text.
sub js_tokens {
    my ($text) = @_;
    my @tokens;
    my $length = length $text;
    my $at = 0;
    my $regex_allowed = 1;
    my %operator_words = map { $_ => 1 } qw(return typeof case do else in of new delete void throw instanceof yield await);
    while ($at < $length) {
        my $char = substr($text, $at, 1);
        if ($char =~ /\s/) { $at++; next; }
        if (substr($text, $at, 2) eq '//') {
            my $end = index($text, "\n", $at);
            $at = $end < 0 ? $length : $end;
            next;
        }
        if (substr($text, $at, 2) eq '/*') {
            my $end = index($text, '*/', $at + 2);
            $at = $end < 0 ? $length : $end + 2;
            next;
        }
        if ($char eq '"' || $char eq "'") {
            my $value = '';
            $at++;
            while ($at < $length && substr($text, $at, 1) ne $char) {
                my $next = substr($text, $at, 1);
                if ($next eq '\\') { $value .= substr($text, $at, 2); $at += 2; next; }
                last if $next eq "\n";
                $value .= $next;
                $at++;
            }
            $at++;
            push @tokens, ['string', $value];
            $regex_allowed = 0;
            next;
        }
        if ($char eq '`') {
            my ($value, $end) = js_template($text, $at + 1);
            $at = $end;
            push @tokens, ['string', $value];
            $regex_allowed = 0;
            next;
        }
        if ($char eq '/' && $regex_allowed) {
            $at++;
            my $class = 0;
            while ($at < $length) {
                my $next = substr($text, $at, 1);
                if ($next eq '\\') { $at += 2; next; }
                last if $next eq "\n";
                $class = 1 if $next eq '[';
                $class = 0 if $next eq ']';
                $at++;
                last if $next eq '/' && !$class;
            }
            $at++ while $at < $length && substr($text, $at, 1) =~ /[A-Za-z0-9_\$]/;
            $regex_allowed = 0;
            next;
        }
        pos($text) = $at;
        if ($text =~ /\G([A-Za-z_\$\x80-\xff][A-Za-z0-9_\$\x80-\xff]*)/gc) {
            my $word = $1;
            $at += length $word;
            push @tokens, ['name', $word];
            $regex_allowed = exists $operator_words{$word} ? 1 : 0;
            next;
        }
        pos($text) = $at;
        if ($text =~ /\G([0-9][0-9A-Za-z_.]*)/gc) {
            $at += length $1;
            push @tokens, ['number', $1];
            $regex_allowed = 0;
            next;
        }
        push @tokens, ['punct', $char];
        $at++;
        $regex_allowed = $char =~ /[)\]}]/ ? 0 : 1;
    }
    return @tokens;
}

# Reads a template literal from just after its opening backtick, returning its raw text and the
# offset after its closing backtick. Substitutions may hold strings, templates, and braces.
sub js_template {
    my ($text, $at) = @_;
    my $length = length $text;
    my $value = '';
    while ($at < $length) {
        my $char = substr($text, $at, 1);
        if ($char eq '\\') { $value .= substr($text, $at, 2); $at += 2; next; }
        return ($value, $at + 1) if $char eq '`';
        if (substr($text, $at, 2) eq '${') {
            my $depth = 1;
            my $start = $at;
            $at += 2;
            while ($at < $length && $depth > 0) {
                my $next = substr($text, $at, 1);
                if ($next eq '`') { (undef, $at) = js_template($text, $at + 1); next; }
                if ($next eq '"' || $next eq "'") {
                    $at++;
                    while ($at < $length && substr($text, $at, 1) ne $next) {
                        $at += substr($text, $at, 1) eq '\\' ? 2 : 1;
                    }
                    $at++;
                    next;
                }
                $depth++ if $next eq '{';
                $depth-- if $next eq '}';
                $at++;
            }
            $value .= substr($text, $start, $at - $start);
            next;
        }
        $value .= $char;
        $at++;
    }
    return ($value, $at);
}
