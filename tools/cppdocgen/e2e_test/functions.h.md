# \<e2e_test/functions.h\> in e2e

[Header source code](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/tools/cppdocgen/e2e_test/functions.h)

## Custom title function. {:#CustomTitleFunction}

[Declaration source code](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/tools/cppdocgen/e2e_test/functions.h#16)

<pre class="devsite-disable-click-to-copy">
<span class="typ">double</span> <b>CustomTitleFunction</b>(<span class="typ">int</span> a,
                           <span class="typ">int</span> b);
</pre>


This function has a custom title and two parameters.


## GetStringFromVectors(…) {:#GetStringFromVectors}

[Declaration source code](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/tools/cppdocgen/e2e_test/functions.h#22)

<pre class="devsite-disable-click-to-copy">
<span class="typ">std::string</span> <b>GetStringFromVectors</b>(<span class="typ">const std::vector&lt;int&gt; &amp;</span> vector1 = std::vector<int>(),
                                 <span class="typ">std::vector&lt;double, std::allocator&lt;double&gt;&gt; *</span> v2 = {},
                                 <span class="typ">int</span> max_count = -1);
</pre>

This documentation uses three slashes and the function has default params with templates.

This also has a link to the <code><a href="functions.h.md#CustomTitleFunction">CustomTitleFunction</a>()</code> which should get rewritten, and
the [FIDL wire format](/docs/reference/fidl/language/wire-format) which should not.


## ThisShouldHaveNoDeclaration() {:#ThisShouldHaveNoDeclaration}

[Declaration source code](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/tools/cppdocgen/e2e_test/functions.h#30)

This function should have no emitted declaration because of:


## UndocumentedFunction() {:#UndocumentedFunction}

[Declaration source code](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/tools/cppdocgen/e2e_test/functions.h#11)

<pre class="devsite-disable-click-to-copy">
<span class="typ">void</span> <b>UndocumentedFunction</b>();
</pre>


