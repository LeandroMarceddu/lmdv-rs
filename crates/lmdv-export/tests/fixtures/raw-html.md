# Raw HTML fixture

<table>
  <thead>
    <tr>
      <th align="left">Item name</th>
      <th align="center">Unit count</th>
      <th align="right">Total cost</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td>Alpha row</td>
      <td colspan="2" align="center">Spanning value</td>
    </tr>
    <tr>
      <td><strong>Beta row</strong></td>
      <td>Twenty two</td>
      <td>Thirty three</td>
    </tr>
  </tbody>
</table>

<table>
  <thead>
    <tr><th>Plain column</th><th>Second column</th></tr>
  </thead>
  <tbody>
    <tr><td>Plain one</td><td>Plain two</td></tr>
  </tbody>
</table>

<table>
  <tbody>
    <tr><td rowspan="2">Rowspan label</td><td>First value</td></tr>
    <tr><td>Second value</td></tr>
  </tbody>
</table>

<ul>
  <li>First bullet</li>
  <li>Second bullet
    <ol start="3">
      <li>Nested third</li>
      <li>Nested fourth</li>
    </ol>
  </li>
</ul>

<details>
<summary>More information</summary>
Body text with <code>inline code</code>.
</details>

<details>
<summary>Only a summary</summary>
</details>

<pre><code>let answer = 42;</code></pre>

<blockquote>Quoted paragraph</blockquote>

<div>
  Video <video controls src="./media/clip.mp4"></video> and audio <audio src="./media/song.mp3"></audio>.
</div>

<audio src="https://example.com/remote.mp3"></audio>

<custom-tag>Fallback paragraph</custom-tag>
